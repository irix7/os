# IRIX 7 — Architectural Design Decision Research Report

**Scope:** every major OS design decision for IRIX 7 — a GPL-3.0, clean-room, Rust-native operating system for the SGI Indy (IP22, MIPS III, big-endian ELF32) that runs existing IRIX 6.5 o32/n32 binaries unchanged through a legacy ABI compatibility container, offers new Rust programs a native capability/object interface, and boots via the factory SGI PROM.

**Status of this document:** research input to ADR-0010 (kernel architecture), ADR-0011 (native capability substrate + compat container), ADR-0016 (o32 ECOFF bootstrap), ADR-0017 (modern equivalents as references), ADR-0018 (codegen backend), and the proposed two-phase ROM handoff.

**Date:** 2026-10-20.

**Method:** Each decision area states the decision question, enumerates the viable options (not only the obvious one), gives reasons for and against each with citations to primary sources (papers, project documentation, source repositories, blog posts), then recommends an option and states the justification and the residual risks. Citations are inline as URLs; a consolidated source list is in Appendix A. Where a claim rests on project documentation that this report could not re-fetch at time of writing, the project is named so the claim can be checked.

**A note on the evidence base:** the strongest primary sources for this report are the Asterinas/Ostd paper (USENIX ATC 2025, arXiv:2506.03876), the Theseus OS book and repository, the seL4 reference manual and verification literature, the Fuchsia/Zircon object-model documentation, the two independent Rust-on-IRIX ports (`danifunker/rust-irixlibstd` and `atomchild411/rust-irix`), the smoltcp documentation, and the Git LFS specification and GitHub API documentation. The IRIX 7 project's own `docs/spec/architecture.md` and `docs/map.md` already encode several decisions; this report tests them against alternatives rather than restating them.

---

## Table of contents

- [A. Kernel structure and architecture](#a-kernel-structure-and-architecture)
  - A.1 Monolithic vs microkernel vs exokernel vs unikernel vs multikernel vs framekernel vs cytokernel
  - A.2 Single address space / single privilege level vs hardware-ring protection
  - A.3 Deep crates vs shallow crates
  - A.4 Cell-based modularity and runtime loading of `.o` files
- [B. Native ABI and capability model](#b-native-abi-and-capability-model)
- [C. Compatibility and the legacy layer](#c-compatibility-and-the-legacy-layer)
- [D. Memory management](#d-memory-management)
- [E. Privilege separation and the TCB](#e-privilege-separation-and-the-tcb)
- [F. Boot and bring-up, including the two-phase ROM handoff](#f-boot-and-bring-up-including-the-two-phase-rom-handoff)
- [G. Toolchain and codegen](#g-toolchain-and-codegen)
- [H. Concurrency and scheduling](#h-concurrency-and-scheduling)
- [I. Device drivers](#i-device-drivers)
- [J. Filesystems and identity services](#j-filesystems-and-identity-services)
- [K. Summary of recommendations](#k-summary-of-recommendations)
- [Appendix A. Consolidated sources](#appendix-a-consolidated-sources)

---

## A. Kernel structure and architecture

### A.1 Decision: what shape should the kernel have?

**Decision question.** Should IRIX 7 be a monolithic kernel, a microkernel, an exokernel, a unikernel, a multikernel, a framekernel, or a cell-based "cytokernel"; and how much of the system should run inside the kernel address space?

The constraint set is unusual: one address space is cheap and desirable because the native capability model and the IRIX compat container must compose closely; the Indy is a single-CPU (or dual-CPU) 1990s workstation with a small TLB and limited RAM; the kernel must be clean-room Rust with a small trusted computing base; and the design must still host a legacy ABI that was written for a conventional monolithic UNIX.

#### Option A1 — Traditional monolithic kernel (Linux/IRIX shape)

The entire kernel — scheduler, VM, VFS, network stack, drivers — is linked into one privileged image and runs in one address space.

**Pros.**

- Matches the IRIX 6.5 personality the compat layer must emulate: IRIX itself was a monolithic SGI kernel, and its syscall semantics (sprocs, shared address spaces, `syssgi`, STREAMS) assume kernel-internal access to process and file state. A monolithic shape makes the compat container a translation layer inside the same kernel, not a distributed system.
- No IPC tax. Microkernel IPC is the dominant cost in multi-server systems; Liedtke's L4 work was largely an exercise in making IPC fast enough (https://dl.acm.org/doi/10.1145/224056.224075), and seL4's own performance page (https://sel4.systems/performance.html) shows that even a highly optimised microkernel pays for capability invocation and address-space switches.
- Simplest bootstrap on a PROM-only machine: one image, one entry point, no server graph to bring up before the first syscall can be served.
- Enormous prior art to borrow *ideas* from (not code): Linux, the BSDs, xv6 (https://github.com/mit-pdos/xv6-riscv), and the IRIX architecture itself.

**Cons.**

- The classic monolithic problem: a single driver bug corrupts the whole kernel. On a machine with no IOMMU (see Section I) and no hardware virtualisation, a monolithic C kernel is a security and reliability liability.
- The IRIX 7 project wants *language-level* isolation, which a conventional monolithic C-shaped kernel does not provide by construction.
- Rust-for-Linux-style "Rust modules inside a C kernel" still needs `unsafe` abstractions over C data structures and does not yield a small TCB (the Asterinas paper makes exactly this argument: arXiv:2506.03876).

**Verdict.** Right semantics, wrong trust model if taken literally. The recommended architecture keeps the monolithic *deployment* shape (one image, one address space) but changes the internal trust boundary (framekernel; see A1-vi).

#### Option A2 — Microkernel with userspace servers (seL4, Redox, MINIX 3, QNX, HelenOS, Genode)

A small privileged kernel provides address spaces, threads, IPC, and capability invocation; drivers, filesystems, and the legacy personality run as userspace servers.

**Pros.**

- Strong fault isolation: a crashed filesystem server is restartable; a compromised driver cannot touch the kernel. MINIX 3 (https://www.minix3.org/) was explicitly built around this; QNX has shipped it in cars for decades.
- seL4 provides the strongest formal story in the field: functional correctness, integrity, and (on AArch64 as of 2026) confidentiality proofs (https://sel4.systems/Verification/, https://sel4.systems/news/). The capability model is the reference design for IRIX 7's native ABI.
- Redox OS demonstrates a Rust microkernel with a Unix personality (https://redox-os.org/, https://gitlab.redox-os.org/redox-os/kernel), and its `relibc` shows how a C library can be rebuilt over a capability kernel.
- Servers can be written in safe Rust and restarted independently.

**Cons.**

- IPC and address-space-switch overhead on a 1990s MIPS CPU with a small TLB is exactly the wrong place to spend cycles. Redox's own documentation and community discussion acknowledge the performance cost of userspace drivers; the IRIX 7 spec already rejects this route for that reason (`docs/spec/architecture.md` §10).
- The compat container would become a userspace server issuing IPC for every legacy syscall — a per-syscall round trip on top of an already-slow emulated Indy.
- Bootstrapping a server graph from the SGI PROM is materially more complex than booting one image.
- A microkernel does not by itself give a small TCB unless the kernel is verified; seL4's kernel is C and its proofs do not transfer to Rust (https://sel4.systems/Verification/).

**Verdict.** Rejected as the deployment shape. Its capability *concept* is adopted (Section B), and its fault-isolation goal is recovered inside one address space by language-level privilege separation (A1-vi).

#### Option A3 — Exokernel (MIT ExOS/XOK, phosphene, KVI)

The kernel only multiplexes hardware securely; libraries implement OS abstractions per application.

**Pros.**

- Maximum flexibility and minimal kernel abstraction: Engler's exokernel work (https://pdos.csail.mit.edu/papers/exokernel-sosp95/; https://pdos.csail.mit.edu/archive/exo/) showed that application-level resource management can beat monolithic general-purpose policies.
- Strong conceptual fit with a capability model: the exokernel hands out capabilities to physical resources.
- Small kernel, so a small TCB in principle.

**Cons.**

- The library OS must reimplement *everything* per application; on IRIX 7 the legacy binaries cannot be changed, so the "library OS" would have to be a single universal one — which collapses back into a monolithic kernel with extra indirection.
- No production-quality Rust exokernel exists to borrow structure from; research prototypes (KVI, phosphene) are not maintained as general-purpose bases.
- Resource revocation and sharing policies become application policy problems, which is exactly what a compat layer for 1990s software cannot tolerate.

**Verdict.** Rejected. Its capability-to-resource idea survives inside the native object model.

#### Option A4 — Unikernel (MirageOS, IncludeOS, Unikraft, Rumprun)

A single-purpose image linked with a library OS, running in one address space at one privilege level.

**Pros.**

- The unikernel literature is the closest prior art for "one address space, no ring transitions": Madhavapeddy et al., "Unikernels: Library Operating Systems for the Cloud" (ASPLOS 2013, https://dl.acm.org/doi/10.1145/2451116.2451166), and Unikraft (https://unikraft.org/, https://github.com/unikraft/unikraft) demonstrate small images and fast boot.
- The single-address-space, single-privilege-level execution model is exactly what IRIX 7 wants for its native path.
- Rust unikernels exist (e.g. https://github.com/hermit-os/hermit-rs, a Rust unikernel) and prove the toolchain path.

**Cons.**

- Unikernels are single-application by design; IRIX 7 must multiplex many legacy processes with `fork`, `exec`, shared memory, and sprocs. A unikernel has no natural process model.
- No demand paging, no swapping, no multi-process isolation story in the classic unikernel shape — all required by IRIX binaries.
- The "library OS" is statically linked per image; the IRIX 7 kernel must load arbitrary existing ELF/ECOFF binaries at runtime.

**Verdict.** Rejected as a whole-system shape, but its single-address-space execution discipline is retained for the native side (A2).

#### Option A5 — Multikernel (Barrelfish)

Treat the machine as a network of cores; each core runs its own kernel instance and cores communicate by message passing.

**Pros.**

- Scales better than shared-memory kernels on many-core hardware: Baumann et al., "The Multikernel: A New OS Architecture for Scalable Multicore Systems" (SOSP 2009, https://www.barrelfish.org/publications/barrelfish_sosp09.pdf; https://barrelfish.org/).
- Explicit state replication and message-passing invariants are a good discipline for SMP correctness.

**Cons.**

- The Indy is single-CPU in its common configuration (IP22 with one R4400), and at most dual-CPU in later Indigo2/Challenge configurations; a multikernel's core complexity buys nothing.
- Message-passing between cores on a machine with no coherent interconnect optimisation is pure overhead.
- No Rust multikernel of production quality exists.

**Verdict.** Rejected for IRIX 7. Its discipline (per-CPU run queues, explicit cross-CPU messages) is retained where SMP exists (Section H).

#### Option A6 — Framekernel (Asterinas/Ostd)

A small privileged *framework* (the only code allowed to use `unsafe`) provides safe abstractions for CPU state, memory frames, page tables, MMIO, DMA, and interrupts; all OS services — including the Linux-compatible syscall layer — are safe Rust built on those abstractions.

**Pros.**

- The strongest published TCB result for a general-purpose Rust OS: ~14.0% of the codebase is memory-safety TCB, versus 44–66% for Tock/Theseus/RedLeaf per the Asterinas paper (arXiv:2506.03876, USENIX ATC 2025).
- Performance on par with Linux in the paper's measurements (mean normalised LMbench 1.08; Redis 1.31; Nginx 1.17), i.e. no microkernel-style IPC tax.
- It is *designed* for exactly IRIX 7's problem: a Linux-ABI-compatible OS with a small sound TCB, implemented in safe Rust on top of a framework. Replace "Linux ABI" with "IRIX o32/n32 ABI" and the architecture transfers.
- Safe policy injection (scheduler, frame allocator, slab allocator as traits) keeps policy out of the TCB — directly useful for IRIX real-time scheduling (Section H).
- The invariants are published and enumerable (Inv. 1–10 in the paper; restated in IRIX 7's spec §3.3), which makes them auditable.

**Cons.**

- Young: Asterinas is a research OS (https://github.com/asterinas/asterinas), not a shipping kernel; its MIPS support does not exist and its abstractions are x86-64-centric in places.
- The framework's safe API surface must be re-derived for MIPS III (TLB, exception vectors, cache management, no IOMMU), which is real engineering, not a port.
- A framekernel still runs one address space unless combined with SAS/SPL or per-process page tables; it constrains *who may use `unsafe`*, not *who may touch what memory*.
- The 14% figure is for Asterinas' codebase; IRIX 7's ratio will differ and must be measured, not assumed.

**Verdict.** **Recommended as the trust architecture.** It is the best-fit published design for "monolithic Rust kernel with a small sound TCB", and it composes with the SAS/SPL and cell ideas below.

#### Option A7 — "Cytokernel" / cell-based OS (Theseus)

The OS is a set of cells (≈ crates) with fine-grained, language-enforced state management; cells can be loaded, unloaded, and replaced at runtime; the kernel itself is just the first set of cells.

**Pros.**

- Theseus demonstrates live evolution and fault recovery without redundancy (https://github.com/theseus-os/Theseus; https://theseus-os.github.io/Theseus/book/index.html). The "intralingual design" thesis — shift resource management into the compiler via affine types — is the most direct expression of IRIX 7's language-level isolation goal.
- Cells map one-to-one to Rust crates, so the build system, the module system, and the runtime namespace coincide.
- Runtime loading of relocatable `.o` files gives post-boot extensibility without a microkernel.
- Theseus's `MappedPages` abstraction (exclusive ownership of a virtually-contiguous region) is a clean basis for memory safety (Section D).

**Cons.**

- Theseus is research-grade and x86-64-only; its cell loading and namespace machinery assumes System V ELF relocations and a particular boot path.
- The "everything is a cell" model has no strong story for untrusted native code — it assumes all cells are trusted Rust. IRIX 7 must host untrusted legacy binaries, so it needs a real boundary somewhere (hence the framekernel split).
- Dynamic unloading of Rust code with live references is subtle; Theseus handles it with extensive runtime bookkeeping whose overhead on a 1990s MIPS core is unmeasured.

**Verdict.** **Recommended as the modularity/deployment model, subordinate to the framekernel trust split**: cells ≈ crates, but only the privileged framework may use `unsafe`, and legacy code runs in a compat container that is itself a de-privileged cell.

#### A.1 comparison table

| Option | Isolation mechanism | IPC/transition cost | TCB size | Fit to IRIX semantics | Rust maturity | Verdict |
|---|---|---|---|---|---|---|
| Monolithic (Linux/IRIX shape) | Hardware rings + MMU | None (syscall only) | Whole kernel | Excellent | High (Rust-for-Linux) | Semantics kept, trust model replaced |
| Microkernel (seL4/Redox/QNX) | Address spaces + capabilities | High on MIPS | Small kernel, large server set | Poor without a server graph | Medium (Redox) | Rejected |
| Exokernel | Capabilities to raw resources | Low kernel, high library | Very small | Very poor (legacy binaries fixed) | Low | Rejected |
| Unikernel | Single AS, single app | None | Small | Poor (no process model) | Medium (Hermit) | Rejected |
| Multikernel | Per-core kernels + messages | High | Small per core | Poor (single CPU) | None | Rejected |
| **Framekernel (Asterinas/Ostd)** | **`unsafe` confinement + safe APIs** | **None** | **~14% published** | **Good (ABI layer is safe Rust)** | **Research-grade but real** | **Recommended (trust)** |
| **Cytokernel/cells (Theseus)** | **Language + cell namespace** | **None** | **Small** | **Good (modular services)** | **Research-grade** | **Recommended (modularity)** |

**Recommendation A.1.** A **framekernel realised as a cell-structured monolithic kernel**: Theseus-style cells ≈ crates for modularity and runtime composition, Asterinas-style privileged framework for the `unsafe`/TCB boundary, and a single kernel image in a single address space for deployment. This is the combination already sketched in the project's `docs/spec/architecture.md` §3, and the research supports it: it keeps the monolithic performance and IRIX semantics while obtaining the smallest published Rust TCB and a credible live-evolution story.

**Open questions and risks.**

- The Asterinas TCB percentage must be re-measured for IRIX 7; the MIPS framework will be a larger fraction than on x86-64 because TLB/cache/exception handling is more intricate.
- Cell unloading with live Rust references needs a formal argument (Theseus relies on runtime bookkeeping; a MIPS port must not weaken it).
- The framework/compat boundary must be drawn so that the compat container never needs `unsafe`; if it does, the TCB grows and the architecture's main claim weakens.
- Asterinas and Theseus are both x86-64-first; neither has a MIPS backend, so "adopt the architecture" means re-implementing the framework ideas, not porting code.

---

### A.2 Decision: single address space / single privilege level, or hardware rings and per-process address spaces?

**Decision question.** Should isolation be enforced by the MMU and CPU privilege modes (traditional rings, per-process page tables), by the Rust type system in one address space at one privilege level (SAS/SPL), or by a hybrid?

#### Option A2-i — Traditional: kernel/supervisor ring, per-process user address spaces

**Pros.**

- Hardware-enforced: a bug in safe *or* unsafe code cannot silently write another process's memory if the MMU is configured correctly.
- Required by the legacy side anyway: IRIX binaries expect `fork`, `exec`, `mmap`, `sbrk`, and per-process address spaces; the MIPS TLB exists and works.
- Mature, well-understood, testable; every UNIX does it.

**Cons.**

- Every syscall and every page fault costs a mode switch and TLB work; on an R4400 with a 48-entry TLB (per SGI IP22 documentation) this is not free.
- Hardware protection does not protect against kernel bugs: a monolithic kernel with a wild pointer still corrupts everything.
- It forces the native capability model and the compat container across a boundary that the project wants cheap (function calls and shared memory).

#### Option A2-ii — Pure SAS/SPL: one address space, one privilege level, language safety only (Theseus)

**Pros.**

- Cheapest possible composition: native capability calls, compat translation, drivers, and services are all function calls in one address space. No mode switches, no IPC, no TLB shootdowns for kernel entry.
- Theseus's thesis: with a safe language, hardware protection is largely redundant for *memory* safety; the compiler enforces what rings would (https://theseus-os.github.io/Theseus/book/index.html; the P.I.E./PHIS principle in the Theseus book).
- Makes runtime cell loading and replacement tractable: no address-space surgery.
- On a machine with no IOMMU and no virtualisation, hardware isolation is weak anyway; leaning on the compiler is rational.

**Cons.**

- One `unsafe` bug anywhere in the TCB defeats all isolation — hence the framekernel split is essential, not optional.
- Legacy binaries are *not* Rust and cannot be trusted by the type system. A pure SAS/SPL design has no answer for "run this 1995 IRIX binary that writes through a wild pointer" except to trust it.
- DMA-capable devices can write anywhere in a single physical address space; without an IOMMU, only careful DMA allocation policy helps (Section I).
- Theseus itself notes that its model assumes all code is trustworthy Rust; it is not designed for hostile native code.

#### Option A2-iii — Hybrid: SAS for native/kernel code, MMU-isolated address spaces only for legacy processes

**Pros.**

- Gets the composition benefits where they matter (native capability objects, compat *translation*, drivers, services all in one space) while still containing untrusted IRIX binaries in their own page tables.
- The compat container can run *in* the kernel address space as the translation layer, while the legacy *application* runs in a user address space — the binary's wild writes hit its own mapping, not the kernel.
- Keeps the door open for a future "native process" mode if hostile native code ever appears.
- Matches the actual threat model: the legacy binaries are the untrusted, non-Rust code; native Rust code is trusted by construction.

**Cons.**

- More complex than either pure option: two worlds, two sets of entry/exit paths.
- The boundary between "compat container in kernel space" and "legacy app in user space" must be designed carefully or the container becomes a confused deputy.
- Some IRIX features (sprocs sharing an address space, `mmap` of devices, shared arenas) need the MMU anyway, so the "pure SAS" simplicity was never fully available.

#### A.2 comparison table

| Option | Memory safety source | Syscall/entry cost | Legacy binary containment | Complexity | Fit |
|---|---|---|---|---|---|
| Hardware rings + per-process AS | MMU + rings | Mode switch + TLB | Strong | Medium | Traditional; ignores language safety |
| Pure SAS/SPL (Theseus) | Rust types only | None | **None** (unsafe legacy code trusted) | Low | Good for native, unsafe for legacy |
| **Hybrid: SAS for native/kernel, MMU for legacy apps** | **Rust types + MMU where needed** | **None for native; mode switch only on legacy syscall** | **Strong** | **Medium-high** | **Best fit** |

**Recommendation A.2.** **Hybrid.** Kernel, framework, native capability substrate, drivers, and the compat *container* live in one address space at one privilege level; legacy IRIX processes get their own MMU address spaces, and the compat container mediates their syscalls. This preserves the Theseus/Asterinas composition benefits for everything written in Rust while refusing to trust 1990s binaries to the type system. It is a refinement of the project spec's pure SAS/SPL stance: the spec's §3.1 claim ("the entire kernel … executes at one privilege level") holds for kernel code; user processes are the deliberate exception, because they are the one category of code the compiler did not verify.

**Open questions and risks.**

- Where exactly does the compat container run — kernel space (fast, trusted) or a privileged user process (safer, slower)? Recommendation: kernel space, because it is clean-room Rust and inside the framekernel's de-privileged tier.
- The MIPS TLB is software-managed; a hybrid design must implement TLB refill for legacy processes and keep the kernel's direct-mapped region fast.
- Signals, `fork`, and sproc semantics cross the boundary; the container must own the process model (Section C).

---

### A.3 Decision: deep crates or shallow crates?

**Decision question.** Should the kernel be a small number of large crates with broad internal modules (shallow), or many small crates with narrow, explicitly-typed interfaces (deep)?

#### Option A3-i — Shallow: few large crates

**Pros.**

- Fewer cross-crate boundaries; rustc's cross-crate inlining is less of an issue; simpler dependency graph.
- Faster incremental builds in some configurations.
- Less interface design work.

**Cons.**

- Isolation is weaker: a large crate is one `unsafe` audit unit, and the framekernel boundary becomes a module convention rather than a compiler-enforced crate boundary.
- Cell-based runtime loading (A.4) requires small relocatable units; a monolith crate cannot be loaded/unloaded piecewise.
- The Asterinas result depends on a sharp framework/services split, which is easiest to enforce across crate boundaries.

#### Option A3-ii — Deep: many small crates with narrow interfaces (Theseus, Asterinas)

**Pros.**

- Each crate is a natural audit and TCB unit; `#![forbid(unsafe_code)]` can be applied per crate, making the framekernel boundary mechanical.
- Maps one-to-one to cells (Theseus), so runtime loading, replacement, and dependency tracking work at crate granularity.
- Forces interface design (the "deep module" discipline): narrow, well-typed seams are easier to verify and to evolve.
- Enables parallel compilation and per-crate feature gating (e.g. a `no-alloc` core set for the ROM phase).

**Cons.**

- More crates means more Cargo/workspace machinery, more version skew, and more cross-crate trait-object or monomorphisation decisions.
- Rust's cross-crate optimisation is weaker than intra-crate; `-Z build-std` and LTO mitigate but do not eliminate this, especially with `rustc_codegen_gcc`.
- Runtime cell loading needs relocation processing per crate, which is more work with many small objects.

#### A.3 comparison table

| Option | TCB auditability | Cell loading | Build complexity | Compiler optimisation | Fit |
|---|---|---|---|---|---|
| Shallow (few large crates) | Weak (module conventions) | Poor | Low | Best | Poor |
| **Deep (many small crates)** | **Strong (`forbid(unsafe_code)` per crate)** | **Excellent** | **Medium-high** | **Good with LTO** | **Best** |

**Recommendation A.3.** **Deep crates.** ADR-0010 already mandates this, and the research supports it: deep crates make the framekernel boundary enforceable by the compiler (`#![forbid(unsafe_code)]` on every de-privileged crate), give cells their natural unit, and make the TCB measurable by counting crates that permit `unsafe`. The build-cost objection is real but manageable with workspace-level LTO and `-Z build-std`.

**Open questions and risks.**

- Cross-crate monomorphisation with `rustc_codegen_gcc` is less battle-tested than with LLVM; measure code size and performance on MIPS before committing to very fine granularity.
- The crate graph must be acyclic at the cell level (Theseus's namespace assumes a DAG); cyclic service dependencies need an explicit callback/registry pattern.
- Decide early which crates are `no_alloc` so the ROM (Section F) can share code with the kernel.

---

### A.4 Decision: cell-based modularity and runtime loading of `.o` files?

**Decision question.** Should the kernel support Theseus-style cells — one crate per relocatable ELF object, loaded and linked at runtime into a namespace — or a conventional statically-linked image with no runtime loading?

#### Option A4-i — Static image only

**Pros.**

- Simplest: the linker resolves everything; no relocation processing, no symbol namespace, no runtime loader in the TCB.
- Best code generation (whole-program LTO, no PLT/GOT indirection).
- Easiest to verify: the loaded image is exactly what was built.

**Cons.**

- No live evolution, no fault recovery by reload, no post-boot driver/module loading.
- Every change requires a reboot — painful on an emulated Indy and contrary to the Theseus-inspired architecture.
- The "cells ≈ crates" identity becomes a source-organisation convention only.

#### Option A4-ii — Runtime cell loading (Theseus model)

**Pros.**

- Live evolution: swap a filesystem or driver cell without rebooting; Theseus demonstrates this (https://theseus-os.github.io/Theseus/book/index.html).
- Fault recovery: reload a failed cell; the namespace tracks dependents.
- Small units: each `.o` is a crate, so dependency metadata comes from ELF relocations.
- Enables multiple OS personalities later (IRIX compat as one cell set, native as another).

**Cons.**

- The loader/linker becomes part of the TCB: it parses untrusted-ish object files and patches code. Theseus's loader is sophisticated and x86-64/System V-specific.
- MIPS relocation types differ (R_MIPS_26, R_MIPS_HI16/LO16, GP-relative relocations); a MIPS cell loader must handle them, and `-mno-abicalls`/static relocation helps but does not remove the work.
- Unloading code with live references requires runtime bookkeeping (Theseus's `Cell`/`CrateNamespace`); getting this wrong is a memory-safety hole in the TCB.
- Runtime linking overhead at boot on a slow CPU; mitigated by prelinking or loading only what is needed.

#### A.4 comparison table

| Option | Flexibility | TCB cost | MIPS effort | Boot time | Fit |
|---|---|---|---|---|---|
| Static image only | Low | None | Low | Best | Safe default |
| **Cells with runtime loading** | **High** | **Loader in TCB** | **Medium-high (MIPS relocs)** | **Some** | **Strategic goal** |

**Recommendation A.4.** **Cells as the module unit from day one, with runtime loading as a staged capability.** Build the kernel as cells (crates → `.o`) so the structure is right, but ship the tracer bullet and early milestones as a statically linked image; add the runtime loader once the MIPS relocation set and the namespace invariants are proven. This gets the architectural benefits without putting an unproven loader in the early TCB.

**Open questions and risks.**

- MIPS relocation handling (HI16/LO16 pairing, GP-relative, 26-bit jumps) is the main technical risk; prototype it early against `rustc_codegen_gcc` output.
- Whether a cell can be unloaded safely while references exist is the Theseus open problem; a conservative "quiesce then unload" protocol is advisable.
- The cell namespace must not become a covert channel or an unchecked symbol-resolution path; symbol visibility should be explicit.

---

### A.5 Section A summary

| Decision | Recommended | Why |
|---|---|---|
| Kernel shape | Framekernel + cell-structured monolith | Small published TCB (~14%), monolithic performance, IRIX semantics |
| Isolation | Hybrid SAS/SPL for Rust code, MMU for legacy processes | Trust the compiler for verified code, not for 1990s binaries |
| Crate depth | Deep crates with `forbid(unsafe_code)` on services | Makes the TCB boundary mechanical and cell loading natural |
| Cells | Cells ≈ crates; static early, runtime loading later | Architectural correctness withoutearly loader risk |

**Cross-cutting risk for Section A.** The architecture is only as good as the framework/services boundary. If the compat container or a driver needs `unsafe`, the TCB grows and the framekernel claim weakens. Treat every `unsafe` addition as an ADR-level decision with a written justification.

---

## B. Native ABI and capability model

### B.1 Decision: capability/object model, traditional syscalls, or a hybrid?

**Decision question.** What is the native interface for new Rust programs: a seL4-style capability system (object reference + access rights, every operation authorised by a capability), a conventional numbered syscall interface, or a hybrid?

#### Option B1 — Pure capability model (seL4 shape)

A capability is an unforgeable reference to a kernel object plus a rights mask; all operations are invocations on a capability. There is no global "syscall number" namespace; authority is possessed, not requested.

**Pros.**

- Principle of least authority is structural: a process can only do what its capability set permits. seL4's manual defines the model precisely (https://sel4.systems/Info/Docs/seL4-manual-latest.pdf), and the verification story (functional correctness, integrity, confidentiality) depends on it (https://sel4.systems/Verification/).
- No ambient authority: no "root", no path-based filesystem access by default; delegation is explicit and revocable (via minting rights-limited copies and revocation).
- Composition is clean: a channel endpoint, a VMO, or a job handle is just another capability; passing one is a single operation.
- The model is well-studied: KeyKOS, EROS, CapROS, and seL4 form a lineage with published semantics (https://sel4.systems/Research/).
- It matches the project's ADR-0011 wording exactly: "handles + rights, jobs, channels, VMAR/VMO, ports".

**Cons.**

- Unfamiliar to C/UNIX programmers and to the IRIX compat layer, which expects file descriptors, `open("/etc/passwd")`, `fork`, and signals. Every legacy concept needs a capability translation (Section C).
- Capability invocation is a distinct trap/ABI; designing it for MIPS III (registers, message registers, reply objects) is real work.
- Revocation and garbage collection of capabilities are subtle; seL4's `revoke` semantics and the "finalisation" problem are famously intricate.
- Bootstrapping a capability system requires an initial capability distribution policy (who gets what at boot), which is easy to get wrong.

#### Option B2 — Traditional numbered syscalls (UNIX shape)

A trap instruction with a number in `$v0` and arguments in registers/stack, dispatched through a table; the kernel checks credentials on each call.

**Pros.**

- Native fit for the compat layer: IRIX o32/n32 already work this way (Section C), so the compat container can be a thin table translation.
- Simple to implement, document, and test; every UNIX programmer understands it.
- No capability distribution problem: the kernel enforces access by UID/GID/path, which legacy software expects.

**Cons.**

- Ambient authority: any code that can make a syscall can name any object the credential permits; confused-deputy attacks are structural.
- The "native ABI" would then be indistinguishable from the legacy ABI, contradicting ADR-0011's requirement for a distinct capability/object interface.
- Hard to express fine-grained delegation (pass a file to a child with read-only rights) without adding `SCM_RIGHTS`-style side channels.

#### Option B3 — Hybrid: capability objects with a syscall-shaped fast path

Capabilities are the authority model; a small number of generic operations (`invoke`, `copy`, `revoke`, `wait`, `signal`) are the only traps, and typed operations are encoded in the invoked object's interface. Optionally, a convenience layer offers POSIX-like wrappers for native code.

**Pros.**

- Preserves the capability security model while keeping the trap surface tiny (seL4 has ~10 syscalls; https://sel4.systems/Info/Docs/seL4-manual-latest.pdf).
- Lets the compat container implement legacy syscalls as *sequences of capability invocations*, not as a parallel authority system.
- A typed native API can sit on top (Rust traits/methods that lower to `invoke` with an interface ID), giving ergonomic Rust while the wire format stays generic.
- This is what Fuchsia/Zircon effectively does: a small set of object types and a generic `zx_object_*`/channel message protocol with typed wrappers (https://fuchsia.dev/).

**Cons.**

- Two-level design (generic invoke + typed interface) needs a stable interface-ID registry and versioning.
- The convenience POSIX layer can accidentally reintroduce ambient authority if not designed carefully.
- Performance: an extra dispatch level versus a direct syscall table; on MIPS this is a few instructions, likely acceptable.

#### B.1 comparison table

| Option | Authority model | Trap surface | Legacy mapping | Delegation | Security story | Fit |
|---|---|---|---|---|---|---|
| Pure capability (seL4) | Possession + rights | Tiny (~10 ops) | Needs translation | Excellent | Strongest (verified lineage) | Strong, but austere |
| Traditional syscalls | Credentials (UID/GID/path) | Large table | Direct | Weak | Ambient authority | Contradicts ADR-0011 |
| **Hybrid capability + typed invoke** | **Possession + rights** | **Tiny + typed IDs** | **Translation to invocations** | **Excellent** | **Strong** | **Best** |

**Recommendation B.1.** **Hybrid capability model.** Adopt seL4's semantics (capability = object reference + rights; all authority mediated by capabilities) but expose a small generic invocation ABI plus typed Rust interfaces, Zircon-style. This satisfies ADR-0011, gives the compat container a single authority mechanism to translate into, and keeps the trap surface small enough to audit.

**Open questions and risks.**

- The interface-ID registry and its versioning must be designed before the first native program ships; retrofitting versioning is painful.
- Capability revocation semantics (immediate vs lazy) affect the compat container's `close`/`dup` emulation.
- The initial capability distribution (boot policy) needs an explicit, documented rule set.

---

### B.2 Decision: which object set?

**Decision question.** Which kernel object types should exist in the native model — the Fuchsia/Zircon set (handles, jobs, channels, VMAR/VMO, ports, sockets, events, futexes), the seL4 set (untyped, endpoint, notification, CNode, TCB, page table, frame, ASID pool, IRQ handler), or a bespoke minimal set?

#### Option B2-i — Zircon-style object set

Handles with rights; jobs (containers of processes); processes and threads; VMARs (address-region allocators) and VMOs (memory objects); channels (bidirectional message pipes); sockets (stream); ports (packet-based event dispatch); events, futexes, timers, resources.

**Pros.**

- Directly matches the project's stated object list; the Zircon documentation is public and detailed (https://fuchsia.dev/fuchsia/src/concepts/objects; https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/docs/concepts/).
- Handles + rights give a uniform delegation mechanism; `zx_handle_duplicate`/`replace` cover `dup2` semantics.
- Jobs give a natural process-tree container for `fork`/`exec` and for killing a subtree — useful for the compat layer.
- Channels with message queues are a good fit for capability transfer (handles attached to messages).
- VMAR/VMO cleanly separate address-space layout from physical memory, which is exactly what the memory model needs (Section D).

**Cons.**

- The full Zircon set is large; implementing all of it before the first native program is over-scoping.
- Some objects (sockets, streams) duplicate channels; a minimal set could omit them and let userspace libraries build streams on channels.
- Zircon's semantics are tuned for a multi-server OS; some (e.g. port packets) assume a userspace driver model.

#### Option B2-ii — seL4-style minimal object set

Untyped memory, endpoints, notifications, CNodes, TCBs, page tables, frames, ASID pools, IRQ handlers.

**Pros.**

- Minimal and formally grounded; each object has a precise capability rights set.
- Untyped memory is a powerful primitive: all kernel objects are retyped from untyped regions, which makes accounting and isolation exact.
- Endpoints/notifications give a clean IPC and notification model.

**Cons.**

- Very low-level: no jobs, no channels-with-handles, no VMAR; a native Rust programmer would have to build process and memory abstractions in userspace.
- The compat container needs process groups, file descriptors, and shared memory — all of which would be bespoke userspace layers.
- seL4's object model assumes a microkernel with user-level servers; adopting it inside a monolithic kernel imports complexity without the isolation benefit.

#### Option B2-iii — Bespoke minimal set

Handles, jobs, channels, VMAR/VMO, ports only (the five named in ADR-0011), with everything else built in userspace.

**Pros.**

- Smallest implementable surface; matches the ADR text literally.
- Enough to express process containment, IPC, memory, and event dispatch.
- Leaves room to add objects later without breaking the model.

**Cons.**

- Missing events/futexes makes writing a Rust `std`-like runtime painful (blocking, timeouts, wait sets).
- Missing a timer object forces timers through ports, which is workable but awkward.
- Missing sockets means the network stack's native interface is bespoke.

#### B.2 comparison table

| Object set | Size | Native ergonomics | Legacy mapping | Implementation risk | Fit |
|---|---|---|---|---|---|
| Zircon-style | Large | High (well-documented) | Good (jobs↔process groups, handles↔fds) | Medium | Best long-term |
| seL4-style | Small | Low (very low-level) | Poor (no jobs/VMAR) | Medium | Poor for a monolith |
| **Bespoke five + events/futex/timer** | **Small-medium** | **Good** | **Good** | **Low-medium** | **Best staged** |

**Recommendation B.2.** **Start with the ADR-0011 five (handles+rights, jobs, channels, VMAR/VMO, ports) plus events/futexes/timers**, and grow toward the Zircon set as needs appear. Document each object's rights mask and lifecycle. Do not implement the full Zircon set up front; the five-object core is enough for the compat container and the first native programs, and it keeps the TCB small.

**Open questions and risks.**

- Rights-mask design: which rights are orthogonal (read/write/execute/map/duplicate/transfer/inspect/manage) and which are object-specific?
- Job vs process: does a job own memory (VMAR) or does a process? Zircon puts VMARs in processes; choose deliberately because `fork` depends on it.
- Ports: packet format, key/context field, and whether they can carry handles.
- Handle table growth and exhaustion policy (a legacy binary with thousands of fds must not break).

---

### B.3 Decision: stable ABI surface across rustc versions

**Decision question.** How can the kernel export a native ABI that survives rustc upgrades — `abi_stable`, `#[repr(C)]` with hand-written layouts, a custom stable layout/IDL, or freezing the compiler?

#### Option B3-i — `abi_stable`

A crate providing `StableAbi` types, `#[repr(C)]`-plus-layout-checked structs, `RVec`/`RString`/`ROption` equivalents, and vtables for trait objects across compiler versions (https://crates.io/crates/abi_stable; https://docs.rs/abi_stable).

**Pros.**

- Purpose-built for the problem: it checks layouts at load time and provides stable equivalents of `Vec`, `String`, `Option`, `Result`, and trait objects.
- Already cleared by the project's GPL-3.0 licence gate (`docs/map.md`: MIT OR Apache-2.0, compatible).
- Actively used for Rust plugin systems; reduces hand-written marshalling.
- Works with `no_std` (with `alloc`), which the kernel needs.

**Cons.**

- It is a substantial dependency with its own generic machinery; the generated code and compile times grow.
- Its ABI is stable *for abi_stable's version*, not forever; a major version bump can still break.
- Not designed for a kernel's hot path; the vtables and `RVec` indirection may cost on MIPS.
- It does not solve the *syscall* ABI (register conventions); it solves the Rust-to-Rust boundary.

#### Option B3-ii — `#[repr(C)]` + hand-written layouts

Define every cross-boundary type as `#[repr(C)]`, use `extern "C"` functions, and hand-write size/alignment assertions.

**Pros.**

- No dependency; total control; minimal codegen.
- `repr(C)` layout is specified by the Rust reference (https://doc.rust-lang.org/reference/type-layout.html), so it is stable across compilers.
- Easiest to map onto MIPS o32/n32 calling conventions for the compat boundary.

**Cons.**

- Rust's default ABI is *not* stable (https://prev.rust-lang.org/en-US/faq.html#does-rust-have-a-stable-abi); every non-`repr(C)` type that crosses the boundary is a latent bug.
- Hand-writing stable `Vec`/`String`/enum equivalents is tedious and error-prone.
- No load-time layout verification; mismatches show up as memory corruption.

#### Option B3-iii — Custom stable layout / IDL (project-owned)

Define a small IDL (interface definition language) for native interfaces, generate `repr(C)` structs and dispatch stubs, and version the interface IDs explicitly.

**Pros.**

- Tailored to the capability model: interface IDs, method ordinals, and rights checks can be generated consistently.
- No third-party ABI crate in the TCB; the generator output is auditable.
- Can generate both Rust and (later) C bindings for the compat container.

**Cons.**

- Building and maintaining a code generator is a project in itself.
- Easy to reinvent abi_stable badly; the edge cases (enums with payloads, trait objects, lifetimes) are exactly where hand-rolled ABIs fail.
- Adds build-system complexity to an already custom toolchain.

#### B3 comparison table

| Option | Stability guarantee | Dependency weight | Runtime cost | Ergonomics | Fit |
|---|---|---|---|---|---|
| `abi_stable` | Strong (versioned) | Medium-high | Some indirection | High | Best for Rust-Rust |
| `repr(C)` hand-written | Strong if disciplined | None | Minimal | Low | Best for kernel/user boundary |
| Custom IDL | Strong (owned) | Build-time only | Minimal | Medium | Strategic, later |

**Recommendation B.3.** **Use `repr(C)` for the actual kernel trap boundary and `abi_stable` only where rich Rust types must cross a version boundary** (e.g. cell interfaces, native library loading). Do not put `abi_stable` on the hot syscall path; do not hand-roll rich types where `abi_stable` already solves the problem. The trap ABI should be a handful of `repr(C)` structs and register conventions, versioned by an interface ID.

**Open questions and risks.**

- `abi_stable` with `no_std` + `alloc` on MIPS III needs verification; its test matrix is x86-64-heavy.
- The interface-ID registry must be append-only; document the compatibility rule.
- Rust's `repr(C)` enum layout on big-endian MIPS must be asserted (discriminant size, padding) with compile-time tests, as `rust-irixlibstd` does with its 995 layout assertions (https://github.com/danifunker/rust-irixlibstd).

---

### B.4 Decision: how do native programs invoke the kernel?

**Decision question.** Should native Rust programs use a C-style syscall instruction (`syscall` with a number in `$v0`), a capability invocation instruction sequence, or a library call that lowers to either?

#### Option B4-i — C-style syscall instruction for native code too

**Pros.**

- One mechanism for native and legacy code; simplest kernel entry.
- Familiar; easy to document.
- The compat container's translation is trivial (same trap, different table).

**Cons.**

- Reintroduces ambient authority and a large dispatch table, defeating the capability model.
- Native programs would encode authority in numbers, not handles; delegation becomes impossible without side channels.
- Contradicts the ADR-0011 requirement that the native ABI be distinct from the C ABI.

#### Option B4-ii — Capability invocation as the only native trap

A native program calls a typed wrapper (Rust trait/method), which lowers to a generic `invoke(cap, method_id, args...)` trap; the kernel dispatches on the object's type and checks rights.

**Pros.**

- Single authority mechanism; the trap surface is tiny.
- Typed wrappers give ergonomics without exposing the wire format.
- The compat container implements legacy syscalls by composing invocations, so there is one security model.

**Cons.**

- Every native operation is at least one trap; batching/`io_uring`-style submission would need a queue object.
- Debugging is harder (no strace-style syscall names unless the wrappers log).
- The wrapper layer must be `no_std`-friendly and stable.

#### Option B4-iii — Library-call fast path with trap fallback (vDSO-like)

Pure functions (time read, CPU count) execute in userspace from a read-only page; privileged operations trap.

**Pros.**

- Removes traps from hot read-only operations (clock, `getpid`, `sched_yield` hints).
- Well-precedented: Linux vDSO, Fuchsia's vDSO equivalents.
- Keeps the capability model for anything that mutates state.

**Cons.**

- A shared read-only page in a capability system must itself be a capability (a VMO mapped read-only), or it becomes an ambient-authority hole.
- More complexity in the loader and the memory model.

#### B.4 comparison table

| Option | Authority model | Trap count | Ergonomics | Legacy reuse | Fit |
|---|---|---|---|---|---|
| C-style syscall | Ambient | Low | Familiar | Direct | Contradicts ADR-0011 |
| **Capability invoke only** | **Capability** | **One per op** | **Good via wrappers** | **Composition** | **Best** |
| vDSO fast path | Capability (mapped RO VMO) | Fewer | Good | N/A | Useful later |

**Recommendation B.4.** **Capability invocation as the only native trap**, with typed Rust wrappers, and a vDSO-like read-only VMO for clock/counter reads once the memory model supports it. Do not add a C-style syscall path for native code; the compat container is the only place a numbered syscall table should exist, and it translates into invocations.

**Open questions and risks.**

- Message-register convention on MIPS III: how many arguments in registers before falling back to a buffer? The o32/n32 conventions give a starting point (Section C).
- Batching: a submission/completion queue object may be needed for I/O-heavy native programs; design it as an object, not a syscall flag.
- Ensure the compat container's translation cannot escalate rights (a legacy `open` must produce a handle with only the rights the legacy credential model implies).

---

### B.5 Section B summary

| Decision | Recommended | Why |
|---|---|---|
| Model | Hybrid capability (seL4 semantics, Zircon ergonomics) | Satisfies ADR-0011; one authority mechanism for native and compat |
| Object set | Five core + events/futex/timer, grow later | Small enough to build, rich enough for `std`-like runtimes |
| Stable ABI | `repr(C)` at the trap boundary; `abi_stable` for rich cell interfaces | Stability where needed, minimal hot-path cost |
| Native invocation | Capability `invoke` only; typed wrappers; vDSO-like RO VMO later | No ambient authority; distinct from the C ABI |

---

## C. Compatibility and the legacy layer

### C.1 Decision: what form does the IRIX 6.5 compatibility layer take?

**Decision question.** How should IRIX 6.5 o32/n32 binaries be executed: a compat container inside the kernel (syscall translation in the same address space), a userspace compatibility server on a microkernel, a shim over an existing kernel, full-system emulation, or user-mode binary translation?

#### Option C1 — In-kernel compat container (translation layer, same address space)

The container implements the IRIX syscall ABI, ELF/ECOFF loader, and SGI libc surfaces as clean-room Rust, translating each legacy syscall into native capability invocations. Legacy *applications* run in their own MMU address spaces; the container runs in kernel space.

**Pros.**

- Cheapest per-syscall path: a trap into the kernel, a table lookup, a translation to an invocation, and a return. No IPC, no second context switch. On an Indy this matters.
- The container is Rust and can be held to `#![forbid(unsafe_code)]`, staying outside the TCB (Section E).
- Direct access to the process model, VMAR/VMO, and job objects: `fork`, `exec`, `mmap`, and sproc semantics can be implemented natively rather than emulated over a narrower host ABI.
- This is the architecture the project spec already selects (`docs/spec/architecture.md` §5), and it follows the Wine/Darling lesson that syscall-level translation is far cheaper than full emulation.

**Cons.**

- The container is kernel code: a bug in it can corrupt kernel state unless the framework boundary is respected. It must never need `unsafe`.
- It must model IRIX semantics precisely, including quirks (sprocs, `syssgi`, STREAMS, `procntl`, arenas) that have no native equivalent.
- Debugging is harder: a legacy crash is a kernel-side event, and a bad translation can take the machine down.

#### Option C1b — Userspace compatibility server (microkernel style)

The container runs as a userspace server; legacy syscalls are forwarded over IPC.

**Pros.**

- Strong fault isolation: a container crash is a server crash, restarts are possible.
- The server can be developed and tested like an ordinary process.
- Precedented by Wine (userspace, https://www.winehq.org/), Darling (userspace Mach/BSD translation, https://github.com/darlinghq/darling), and WSL1 (a syscall translation layer in the NT kernel, https://learn.microsoft.com/en-us/windows/wsl/about).

**Cons.**

- Every legacy syscall becomes at least two context switches plus IPC serialisation; on a 1990s MIPS CPU this is a large regression.
- The server still needs privileged operations (address-space manipulation, signal delivery), so it needs a privileged helper or a wide capability set — the isolation is partial.
- Bootstrapping order: the server must be running before the first legacy binary, complicating early bring-up.

#### Option C1c — Shim over an existing kernel (hosted IRIX-on-Linux style)

Run the IRIX binaries on a host kernel with a shim (like `ibcs2`, `personality(2)`, or a ptrace-based loader).

**Pros.**

- Least new kernel code; reuse an existing VM/FS/scheduler.
- Fast to prototype.

**Cons.**

- Contradicts the project's reason for existing: IRIX 7 *is* the kernel; there is no host.
- A shim cannot provide IRIX-specific facilities (sprocs, arenas, `hinv`, real-time classes) without deep host changes.
- Not applicable to a bare-metal Indy.

#### Option C1d — Full-system emulation (QEMU-style)

Emulate an Indy and run IRIX 6.5 itself.

**Pros.**

- Perfect compatibility by construction; the project already uses an emulator (`irix7/iris`, a fork of `techomancer/iris`) for testing.
- Zero clean-room ABI work.

**Cons.**

- That is not IRIX 7; it is IRIX 6.5 under emulation, with none of the Rust-native goals.
- Performance is far below native, and the result is not a new OS.
- Useful only as an oracle, which is exactly how the project uses it.

#### Option C1e — User-mode binary translation (QEMU user, FEX, Box64, Rosetta 2 style)

Translate legacy MIPS binaries to native instructions at load time or run them under a CPU emulator in user mode.

**Pros.**

- Can run binaries whose ABI is not fully understood yet; translation and ABI emulation are separable.
- Precedented at scale: Apple's Rosetta 2, FEX-Emu (https://github.com/FEX-Emu/FEX), Box64 (https://github.com/ptitSeb/box64), QEMU user-mode (https://www.qemu.org/docs/master/user/main.html).

**Cons.**

- Translating MIPS III to MIPS III is pointless; the ISA is the same. The hard part is the *ABI and syscall semantics*, which translation does not solve.
- Adds a translator to the TCB and hurts performance.
- Only useful as a debugging fallback for binaries that defeat the loader.

#### C.1 comparison table

| Option | Per-syscall cost | Isolation of container | IRIX semantics fidelity | Effort | Fit |
|---|---|---|---|---|---|
| **In-kernel container** | **One trap** | Container must be safe Rust | High (full kernel access) | High | **Best** |
| Userspace server | Trap + IPC + switch | Strong | Medium (needs privileged helper) | High | Poor on Indy |
| Shim over host kernel | Low | N/A | Low | Low | Not applicable |
| Full-system emulation | Very high | N/A | Perfect | Low (use IRIX) | Oracle only |
| User-mode translation | High | Medium | Medium | Medium | Debug fallback |

**Recommendation C.1.** **In-kernel compat container** (Option C1), with the container itself written in safe Rust and confined to the de-privileged tier. Keep user-mode translation as a debugging tool only, and keep full-system IRIX under the `irix7/iris` emulator as the oracle. This matches the project spec, the performance reality of the Indy, and the clean-room strategy.

**Open questions and risks.**

- The container's interface to the framework must be expressive enough for `fork`, `exec`, `mmap`, signals, and sprocs without `unsafe`; this is the single largest design risk in the compat layer.
- IRIX semantics that have no native analogue (`syssgi`, arenas, `procntl`, STREAMS) need explicit object mappings or emulation.
- A container bug is a kernel bug; fuzzing the syscall decoder and the loader is mandatory.

---

### C.2 Decision: layer order and boundaries

**Decision question.** In what order should the legacy stack be built: syscall ABI first, then loader, then libc — or another order; and where does each layer live?

The project's `docs/map.md` fixes the order as **syscall ABI → loader → libc**. This is the right order, and the research explains why.

**Syscall ABI first.** The trap decoder is the narrowest, most testable seam: it takes registers and memory from a legacy process and produces a typed request. Until it exists, nothing can be observed. The o32/n32 conventions are documented (see C.3) and can be validated against the oracle with tiny hand-written assembly programs.

**Loader second.** The loader turns an on-disk ELF or ECOFF file into a process image: segments, permissions, entry point, interpreter (`/lib/rld` for o32, `/lib32/rld` for n32, per `rust-irixlibstd`'s target table), and initial stack/argv/envp. It depends on the syscall ABI only for `mmap`/`open`/`read` during loading, which can be stubbed initially.

**libc third.** The SGI libc surfaces are the largest surface and the one most likely to hide ABI quirks (varargs, `struct stat` layout, `DIR`, `FILE`, locale, `errno` TLS). Building it last means each function can be tested against a working syscall path. This mirrors `rust-irixlibstd`'s approach: layout assertions first (995 compile-time assertions across o32/n32), then runtime stages (https://github.com/danifunker/rust-irixlibstd).

#### Alternatives considered

- **libc first**: tempting because it is the most visible, but without a syscall path every libc function is a stub; testability is poor.
- **loader first**: possible, but a loader needs `mmap`/`open`/`read`, so it immediately depends on the syscall ABI.
- **All at once**: highest risk; ABI bugs surface as memory corruption with no isolation between layers.

**Recommendation C.2.** Keep the project's order. Add one explicit sub-layer: **ABI layout definitions** (struct offsets, enum values, syscall numbers) as a generated, test-asserted crate *before* the syscall decoder, because both the decoder and libc depend on it. `rust-irixlibstd`'s layout-assertion approach is the model.

**Open questions and risks.**

- Generated layout tables must come from the captured sysroot, not from memory; the clean-room pipeline must record provenance.
- The dynamic linker (`rld`) is itself an ABI surface: IRIX binaries may be dynamically linked against `/lib/rld`; the loader must either implement the rld protocol or provide a compatible rld.
- Static vs dynamic: many IRIX binaries are dynamically linked; the loader must handle `DT_NEEDED`, symbol resolution, and `LD_LIBRARY_PATH`-style environment variables.

---

### C.3 Decision: o32 and n32 syscall conventions

**Decision question.** How should the kernel decode both o32 (MIPS II, ILP32) and n32 (MIPS III, ILP32 with 64-bit registers) syscalls, including the register/stack argument split and the error convention?

**The conventions (as documented by the MIPS ABI and the IRIX ports).**

- **o32:** syscall number in `$v0`; arguments 1–4 in `$a0`–`$a3`; arguments 5–8 on the caller's stack (at `16($sp)`, `20($sp)`, `24($sp)`, `28($sp)` in the standard frame); result in `$v0`; the error indication is a separate flag (the MIPS UNIX tradition uses `$a3`, with IRIX's libc stub interpreting it) — the exact IRIX convention must be pinned from the oracle.
- **n32:** syscall number in `$v0`; arguments 1–6 in `$a0`–`$a5` (64-bit registers, 32-bit data model); further arguments on the stack; result in `$v0`.
- Both are big-endian; n32 uses 64-bit registers with 32-bit pointers, o32 uses 32-bit registers.
- `rust-irixlibstd` confirms the practical split: o32 is MIPS II and n32 is MIPS III, and LLVM cannot compile the o32 ABI against a 64-bit ISA (`-mcpu=mips3` with o32 crashes instruction selection), so o32 implies MIPS II (https://github.com/danifunker/rust-irixlibstd).

#### Option C3-i — Two decoders, one dispatch table

Separate o32 and n32 trap entry paths that normalise into a common `SyscallArgs` struct, then one dispatcher.

**Pros.**

- Each path can be written in the exact register convention, in a few lines of assembly plus Rust.
- The common dispatcher sees a uniform argument array, so syscall implementations are written once.
- Easy to test: the same syscall test binary can be built for both ABIs and must produce identical results.
- Mirrors how Linux/MIPS handles o32/n32/n64 (`arch/mips/kernel/scall32-o32.S`, `scall64-o32.S`) — architecture precedent, not code reuse.

**Cons.**

- Two entry paths to audit; a bug in one may not appear in the other.
- The o32 stack-argument fetch must validate the user stack pointer and copy safely (a malicious/buggy binary can point anywhere).
- Pointer-width differences leak into syscall implementations (`long` vs `int`, `off_t` 32 vs 64).

#### Option C3-ii — Single decoder with runtime ABI flag

One entry path reads a per-process ABI flag and branches.

**Pros.**

- One code path, less duplication.
- The ABI flag is needed anyway (for `uname`, `personality`-like behaviour, and libc selection).

**Cons.**

- The register conventions differ in *where arguments live*, so the branch happens before normalisation anyway; the saving is illusory.
- A mis-set flag silently misdecodes every syscall.

#### Option C3-iii — Trampoline per ABI in userspace

Each ABI's libc stub issues a distinct trap instruction or sets a distinct register, and the kernel has one entry.

**Pros.**

- Kernel entry is uniform; the ABI is selected by the stub.
- The kernel can reject unknown stubs.

**Cons.**

- The legacy binaries' stubs are fixed; the kernel cannot require them to change. Only works if the existing stubs already distinguish the ABI (they do via `$v0` ranges and register width, not via a dedicated flag).
- Adds a userspace dependency to a kernel decision.

#### C.3 comparison table

| Option | Code duplication | Robustness | Legacy compatibility | Fit |
|---|---|---|---|---|
| **Two decoders, one dispatcher** | Medium (entry only) | High (explicit per ABI) | Exact | **Best** |
| Single decoder + flag | Low | Medium (flag bugs) | Exact if flag correct | Acceptable |
| Userspace trampoline | Low | Low (cannot change binaries) | Poor | Rejected |

**Recommendation C.3.** **Two thin decoders normalising into one dispatcher**, with the ABI recorded per process. Validate both against the same test corpus, as `rust-irixlibstd` does with its 62 runtime stages on both ABIs. Treat the o32 stack-argument path as hostile input: bounds-check the stack pointer and copy arguments with `copy_from_user`-style checked access.

**Open questions and risks.**

- The IRIX error convention must be captured exactly; a wrong flag register breaks every libc stub.
- 64-bit `off_t` on n32 vs 32-bit on o32 changes `stat`, `lseek`, and `mmap` layouts; the layout crate must encode both.
- Syscall numbers: IRIX's table differs from Linux's; the clean-room pipeline must capture the numbers as interface facts and record their provenance.
- `syssgi` is a multiplexed syscall with many sub-operations; it needs its own sub-dispatch and is a large compatibility surface.

---

### C.4 Decision: ELF vs ECOFF loading

**Decision question.** Should the kernel load ELF, ECOFF, or both; and which format does the boot path use?

**The constraint.** The SGI PROM (ARCS) boots o32 ECOFF images. IRIX 6.5 user binaries are ELF (o32 and n32). The project's tracer bullet (ADR-0016) is a freestanding Rust kernel reached through an **o32 ECOFF bootstrap**, because ARCS understands ECOFF and o32/MIPS II, not n32 ELF.

#### Option C4-i — ECOFF bootstrap, ELF user binaries (recommended)

The PROM loads an o32 ECOFF bootstrap/kernel image; the kernel then loads ELF o32/n32 user binaries.

**Pros.**

- Matches the hardware/firmware reality: ARCS is ECOFF-only, and o32/MIPS II is the buildable ABI (`rust-irixlibstd`; issue #41).
- Keeps the boot path small: the ECOFF image only needs to set up the machine and jump into the n32 kernel.
- User binaries keep their native ELF format, so the loader is a normal ELF loader.
- Precedented: Linux/MIPS has ECOFF support for exactly this reason (`CONFIG_BINFMT_ECOFF`), and the IRIX PROM's ARCS loader is documented in SGI's boot documentation.

**Cons.**

- Two object formats in one system: ECOFF for boot, ELF for user; two sets of headers, relocations, and section semantics.
- ECOFF tooling is archaic; producing a correct ECOFF image from Rust requires a converter or a custom linker step (binutils supports ECOFF for `mips-sgi-irix`).
- The bootstrap must be built for MIPS II/o32 while the kernel is MIPS III/n32; the handoff must switch the CPU mode cleanly.

#### Option C4-ii — ELF-only, with a PROM shim

Use an ELF image and rely on a small PROM-resident shim or a converted ECOFF wrapper.

**Pros.**

- One format everywhere; simpler loader.
- Modern tooling.

**Cons.**

- The factory PROM will not load ELF; a shim must itself be ECOFF, so ECOFF does not disappear.
- Adds a conversion step that can obscure boot failures.

#### Option C4-iii — ECOFF-only

**Pros.**

- One format, matching the PROM.

**Cons.**

- IRIX 6.5 user binaries are ELF; the loader would have to convert or reject them. This breaks the compatibility mandate.
- ECOFF lacks modern sections/relocations needed for Rust and for dynamic linking.

#### C.4 comparison table

| Option | PROM compatibility | User binary compatibility | Tooling | Complexity | Fit |
|---|---|---|---|---|---|
| **ECOFF boot + ELF user** | **Native** | **Native** | Mixed (binutils ECOFF + ELF) | Medium | **Best** |
| ELF-only + shim | Needs ECOFF shim | Native | Modern | Medium | Workable but indirect |
| ECOFF-only | Native | **Broken** (IRIX 6.5 is ELF) | Archaic | Low | Rejected |

**Recommendation C.4.** **ECOFF bootstrap + ELF user binaries**, exactly as ADR-0016 states. Build the bootstrap as a tiny o32/MIPS II ECOFF image whose only job is to establish a minimal machine state and jump to the n32 kernel. Keep the ECOFF converter in the build pipeline and test the image under the `irix7/iris` emulator.

**Open questions and risks.**

- The ECOFF image's load address and entry point must match what ARCS expects; the PROM's memory map and boot environment variables are the authority.
- The o32→n32 transition: the bootstrap runs with 32-bit registers and MIPS II instructions; the kernel is MIPS III with 64-bit registers. The jump must set up the stack, GP, and any state the n32 code assumes.
- ECOFF relocation support in the chosen linker; binutils 2.47 with the IRIX patches is the project's path.
- Cache coherency and TLB state at handoff: the PROM leaves the machine in a specific state, and the bootstrap must not assume more.

---

### C.5 Decision: reimplemented SGI libc ABI surfaces

**Decision question.** Which libc surfaces must be reimplemented for o32 and n32, and how should their layouts be validated?

**Surfaces (minimum viable compatibility set).**

- Process: `fork`, `exec*`, `wait*`, `exit`, `getpid`/`getppid`, `setuid`/`setgid`, `sproc` (IRIX-specific), `procntl`.
- Memory: `malloc` family (can be a clean-room allocator behind the ABI), `mmap`/`munmap`/`mprotect`, `brk`/`sbrk`, `memalign`, `valloc`.
- Files: `open`/`close`/`read`/`write`/`lseek`, `stat`/`fstat`/`lstat`, `opendir`/`readdir`/`closedir`, `dup`/`dup2`, `fcntl`, `ioctl`, `access`, `chdir`/`getcwd`, `link`/`unlink`/`rename`, `symlink`/`readlink`.
- Time: `time`, `gettimeofday`, `clock_gettime`, `nanosleep`, `select`/`poll`.
- Signals: `signal`/`sigaction`, `sigprocmask`, `kill`, `raise`, `sigsetjmp`/`siglongjmp`.
- Threads: IRIX `pthread` surfaces (and `sproc` for the older model).
- stdio: `FILE`, `printf` family, `fopen`/`fclose`/`fread`/`fwrite`, `setvbuf`.
- Locale/ctype, `errno` (TLS or global depending on ABI), `sysconf`, `uname`.
- Networking: BSD sockets (`socket`/`bind`/`connect`/`accept`/`send`/`recv`), `getaddrinfo`, `gethostbyname`.
- IRIX-specific: `syssgi`, `hinv`/`getinvent`, `sysmp`, `ksym`, `arena`/`mpc` (if in scope), `freelist`.

**Layout validation.** `rust-irixlibstd` generated 995 compile-time assertions (498 o32, 497 n32) that bindings match the ABI, and ran 62 runtime stages on both ABIs (https://github.com/danifunker/rust-irixlibstd). IRIX 7 should do the same, but from its own captured sysroot and with its own generated layout crate. This is the single most effective defence against silent ABI drift.

#### Options for the libc implementation strategy

- **C5-i — Clean-room reimplementation in Rust** (recommended): implement the ABI surface in Rust, using the layout crate and the oracle for behaviour. Pros: licence-clean, type-safe, testable against the oracle; cons: large surface, subtle behaviour (varargs, `FILE` internals) must be reproduced.
- **C5-ii — Port musl/newlib/relibc**: reuse a permissively licensed libc and add an IRIX ABI shim. Pros: fast coverage; cons: licences (musl MIT, newlib BSD-style, relibc MPL-2.0 — check GPL compatibility), and the internal `FILE`/locale behaviour differs from SGI libc, which legacy binaries may depend on. Also, importing a libc is a large provenance/clean-room question.
- **C5-iii — Binary shim over the original IRIX libc**: ship SGI's `libc.so` and intercept syscalls. Pros: perfect ABI; cons: violates the clean-room and GPL-3.0 goals (SGI libc is not redistributable), and defeats the purpose.
- **C5-iv — Hybrid: Rust core + generated stubs**: generate the exported symbol table and struct layouts from the sysroot, implement semantics in Rust, and use the oracle to fill gaps. Pros: systematic coverage; cons: generator complexity, and the generated stubs must not embed SGI expression.

#### C.5 comparison table

| Strategy | Licence cleanliness | ABI fidelity | Effort | Risk | Fit |
|---|---|---|---|---|---|
| **Clean-room Rust** | **Clean** | **High with oracle** | **High** | Behavioural gaps | **Best** |
| Port musl/newlib/relibc | Depends (check licences) | Medium (internals differ) | Medium | `FILE`/locale mismatch | Fallback for leaf functions |
| Original SGI libc | **Violates clean-room** | Perfect | Low | Legal | Rejected |
| Hybrid generated stubs + Rust | Clean if generator is | High | Medium-high | Generator correctness | Useful accelerator |

**Recommendation C.5.** **Clean-room Rust implementation, accelerated by a generated layout/export crate and an oracle-driven behavioural test suite.** Use permissive third-party code only for leaf, well-isolated functions (e.g. `strtod`, `printf` formatting) where the licence gate clears it, and never for stateful internals (`FILE`, locale, `errno`) where SGI behaviour is observable.

**Open questions and risks.**

- `FILE` layout and stdio buffering behaviour are observable to binaries that pass `FILE*` across library boundaries; the ABI must match, not just the functions.
- `errno` placement (TLS vs global) differs between o32 and n32; get it wrong and every error path breaks.
- Varargs calling conventions on MIPS (the `al` register convention for `printf`-style functions) must be implemented exactly; this is a classic source of breakage.
- `sproclib` and `pthread` coexist in IRIX; both may be in use.
- The clean-room pipeline must treat ABI facts (numbers, offsets) as specification inputs with recorded provenance; legal review is advisable for anything beyond interface facts.

---

### C.6 Decision: process model — fork/exec, sprocs, and signals

**Decision question.** How should the container model IRIX processes: classic `fork`/`exec` with copy-on-write address spaces, sprocs (shared address space lightweight processes), or a hybrid?

**Background.** IRIX's `sproc(2)` creates a shared-address-space process (like Linux `clone(CLONE_VM)`), and `pthread` is built on it. IRIX also has the classic `fork`/`exec` model. Both must work for legacy binaries.

#### Option C6-i — Native jobs + VMAR cloning

Represent a process as a job with a VMAR tree; `fork` clones the VMAR (copy-on-write via VMO sharing); `sproc` creates a new thread group sharing the VMAR.

**Pros.**

- Maps cleanly onto the native object model (Section B).
- COW via VMO reference counts is efficient and well-understood.
- sproc becomes a sharing flag, not a separate mechanism.

**Cons.**

- VMAR cloning semantics must match IRIX's `fork` (what is inherited, what is not: fds, signal handlers, timers).
- COW on a software-managed TLB requires careful fault handling.

#### Option C6-ii — Emulate processes entirely in the container

The container keeps its own process table and switches address spaces itself.

**Pros.**

- Full control over IRIX semantics.
- No dependence on native job semantics.

**Cons.**

- Duplicates the kernel's scheduler and VM; two process models to keep consistent.
- The native and legacy worlds diverge, complicating shared memory and signals.

#### C.6 comparison table

| Option | Semantic fidelity | Implementation cost | Native integration | Fit |
|---|---|---|---|---|
| **Native jobs + VMAR cloning** | High | Medium | Excellent | **Best** |
| Container-owned process table | High | High (duplicate VM/scheduler) | Poor | Rejected |

**Recommendation C.6.** **Native jobs + VMAR cloning**, with the container mapping IRIX process attributes onto job/process objects. Implement `fork` as COW VMAR clone, `sproc` as shared-VMAR thread group, and signals as port/notification deliveries with IRIX-compatible semantics (including `sigsuspend`, `sigaltstack`, and the IRIX quirk that `sigaltstack` wants the top of the stack, per `rust-irixlibstd`'s notes).

**Open questions and risks.**

- IRIX `fork` inherits *arenas* and *shared memory* in ways that must be captured from the oracle.
- Signal delivery to sprocs (which thread gets it?) is IRIX-specific; the container must implement IRIX's rule, not Linux's.
- `exec` must reset the process image while preserving the job/handle table per IRIX rules (close-on-exec, etc.).

---

### C.7 Section C summary

| Decision | Recommended | Why |
|---|---|---|
| Container form | In-kernel safe-Rust translation layer | Cheapest per syscall; full semantic access; matches spec |
| Layer order | Layout crate → syscall ABI → loader → libc | Testability; each layer depends on the previous |
| o32/n32 | Two thin decoders → one dispatcher | Exact conventions; shared implementation |
| Object formats | ECOFF boot, ELF user | PROM reality + IRIX 6.5 reality |
| libc | Clean-room Rust + generated layouts + oracle tests | Licence-clean, high fidelity |
| Process model | Native jobs + VMAR cloning; sproc = shared VMAR | Reuses native objects; matches IRIX |

---

## D. Memory management

### D.1 Decision: what is the core memory abstraction?

**Decision question.** Should the kernel's fundamental memory unit be a raw physical frame, a virtually-contiguous owned region (Theseus `MappedPages`), a reference-counted frame/segment pair (Asterinas `Frame`/`Segment`), or capability-addressed untyped memory (seL4)?

#### Option D1 — Raw physical frames + page tables

The kernel allocates 4 KiB frames and maps them; ownership and lifetime are conventions enforced by code.

**Pros.**

- Simplest and most familiar; every UNIX works this way.
- Maximum flexibility: any mapping pattern is expressible.
- Easy to implement on MIPS's software-managed TLB.

**Cons.**

- No language-level guarantee: a dangling mapping or double free is a memory-safety bug, exactly what the project wants to eliminate.
- Refcounting is ad hoc; use-after-free and leaks are the norm.
- Does not compose with Rust references: a `&T` into a mapped frame can outlive the mapping.

#### Option D1b — Theseus `MappedPages`

A `MappedPages` object owns a virtually-contiguous region and the physical frames backing it exclusively; borrowing a `&T` from it is sound because the region cannot be unmapped while the borrow lives (https://theseus-os.github.io/Theseus/book/index.html; https://github.com/theseus-os/Theseus).

**Pros.**

- Ownership is encoded in the type system, so Rust references derived from the region are sound by construction.
- Virtually-contiguous, which suits the MIPS TLB and makes DMA/MMIO windows easier to describe.
- Supports splitting/merging and permission changes with Rust's borrowing rules.
- Theseus demonstrates the model on real hardware (x86-64) and documents the invariants.

**Cons.**

- Exclusive ownership is wrong for shared memory (sprocs, `mmap(MAP_SHARED)`, page cache), which needs reference-counted sharing.
- Theseus's implementation is x86-64-specific; MIPS needs its own page-table/TLB backend.
- Unmapping while references exist must be prevented by API design, not just documentation.

#### Option D1c — Asterinas `Frame`/`Segment` with reference counts

Frames are physical page metadata with reference counts; segments are contiguous frame runs; both are allocated from the frame allocator and can be shared (https://arxiv.org/abs/2506.03876).

**Pros.**

- Reference counting handles sharing (COW, page cache, shared memory) correctly.
- The frame metadata (refcount, mapping state) is the natural place to enforce Inv. 1 (a newly allocated frame comes from unused memory).
- Segments give contiguous physical runs for DMA.
- The Asterinas paper reports this design supports a Linux-ABI-compatible OS at Linux-level performance.

**Cons.**

- Reference counts are runtime state; a missed decrement is a leak, a double decrement is a use-after-free — the safety argument depends on the framework's encapsulation.
- Contiguous segments can fragment; high-order allocation on a small-RAM Indy is a real constraint.
- The paper's implementation is x86-64; MIPS cache/TLB coherency adds work.

#### Option D1d — seL4 untyped memory

All memory starts as untyped capabilities; kernel objects (frames, page tables, TCBs) are retyped from untyped regions; the capability system tracks ownership exactly.

**Pros.**

- The strongest accounting model: no hidden allocation, no fragmentation surprises, precise isolation; the basis of seL4's verification (https://sel4.systems/Info/Docs/seL4-manual-latest.pdf).
- Delegation of memory is just capability delegation.
- No kernel heap in the traditional sense.

**Cons.**

- Extremely low-level for a general-purpose OS; every subsystem must manage its own untyped pools.
- Retyping is not free, and a monolithic kernel with many services would spend effort on pool management.
- The compat container expects `malloc`/`mmap` semantics; mapping them onto untyped memory is possible but indirect.
- No production Rust implementation.

#### D.1 comparison table

| Option | Safety mechanism | Sharing | DMA contiguity | Complexity | Fit |
|---|---|---|---|---|---|
| Raw frames | Conventions | Manual | Manual | Low | Unsafe |
| **Theseus `MappedPages`** | **Ownership types** | **Poor (exclusive)** | **Good (virtually contiguous)** | **Medium** | **Core for private memory** |
| **Asterinas `Frame`/`Segment`** | **Refcounts + encapsulation** | **Good** | **Good (segments)** | **Medium** | **Core for shared/COW** |
| seL4 untyped | Capabilities | Excellent | Manual | High | Accounting model, not whole answer |

**Recommendation D.1.** **Two-layer model: `Frame`/`Segment` with reference counts underneath, `MappedPages` ownership on top.** Use refcounted frames/segments for physical memory and sharing (COW, page cache, shared memory), and expose `MappedPages`-style owned regions for private mappings where Rust references must be sound. This is the combination the project spec sketches (§3.4), and it matches the strengths of both source designs: Asterinas for physical accounting, Theseus for virtual ownership. Adopt seL4's *accounting discipline* (all memory accounted, no hidden allocations) without its full untyped-retype API.

**Open questions and risks.**

- The boundary between `MappedPages` and shared segments must be explicit; a `MappedPages` that is secretly shared breaks the ownership invariant.
- Frame metadata size on a small-RAM machine: a `struct Frame` per 4 KiB page at, say, 64 bytes is 1.6% of RAM; acceptable but must be measured.
- High-order segment allocation on a fragmented Indy may fail; a fallback to scatter-gather DMA (Section I) is needed.

---

### D.2 Decision: frame and segment metadata, reference counting, and invariants

**Decision question.** How should frame/segment metadata be represented, and which invariants should the framework enforce?

**Design.** A `Frame` describes one physical page: physical address, reference count, flags (allocated, mapped, sensitive, DMA-able), and an owner/segment link. A `Segment` describes a contiguous run of frames with a base address and length. The frame allocator hands out frames; the segment allocator hands out runs. Reference counts are incremented on mapping/share and decremented on unmap/drop; zero triggers return to the allocator.

**Invariants to enforce (adapted from Asterinas, `docs/spec/architecture.md` §3.3).**

1. A newly allocated frame/segment originates from currently unused memory.
2. Kernel-mode CPU state cannot be tampered with by clients.
3. Kernel-mode CPU state cannot be tampered with by devices (interrupt remapping/IOMMU-equivalent).
4. Sensitive memory (kernel code, stacks, page tables) cannot be tampered with by clients.
5. Sensitive memory cannot be exposed to user space.
6. Sensitive I/O memory cannot be accessed by devices; DMA only over untyped memory.
7. Sensitive MMIO/PIO cannot be instantiated by clients.
8. A task runs on at most one CPU at a time.
9. A heap slot/object must not outlive its parent slab.
10. An object is created from a heap slot only if the slot meets size/alignment.

**Options for enforcement.**

- **D2-i — Type-state APIs**: `Frame` states (Free → Allocated → Mapped → Freed) encoded in the type system or with `PhantomData`; illegal transitions do not compile. Pros: strong, zero runtime cost; cons: complex generics, and dynamic sharing patterns are hard to express.
- **D2-ii — Runtime checks in the framework**: refcount assertions, `debug_assert!` in the privileged framework, and safe APIs that cannot express illegal states. Pros: simpler, handles dynamic cases; cons: bugs become panics (availability) rather than compile errors.
- **D2-iii — Both**: type-state for the common path, runtime checks at the boundary. Pros: defence in depth; cons: more code.

**Recommendation D.2.** **Both (D2-iii), weighted toward safe APIs that make illegal states unrepresentable**, with runtime assertions in the framework as a backstop. Reference counts should be non-atomic per-CPU where possible and atomic only for cross-CPU sharing, to avoid MIPS atomic-instruction costs (`ll`/`sc` on R4000 are not cheap).

**Open questions and risks.**

- Atomic refcounts on MIPS III: `ll`/`sc` are available but contended cache lines hurt; measure.
- Frame metadata placement: a static array sized at boot (simple, wastes RAM) vs a dynamically allocated map (complex). For the Indy's RAM sizes, a static array is likely fine.
- Leak detection: a periodic audit of allocated frames is cheap insurance.

---

### D.3 Decision: untyped memory for externally-modifiable regions (MMIO, DMA, user-mapped)

**Decision question.** How should the kernel expose memory that external agents can modify — device registers (MMIO), DMA buffers, and user-mapped pages — without breaking Rust's aliasing and validity guarantees?

#### Option D3-i — Raw pointers / `volatile` accesses

**Pros.** Direct, familiar, zero abstraction cost; matches how C drivers work.
**Cons.** Requires `unsafe` everywhere, which defeats the framekernel split; `volatile` does not solve aliasing or validity; a `&mut u32` to a device register is UB-adjacent because the device can change it.

#### Option D3-ii — `IoMem`/`IoPort` wrappers with volatile accessors

A safe wrapper over a mapped MMIO region that only permits `read`/`write` of POD types at checked offsets, with volatile semantics (Asterinas's approach; see arXiv:2506.03876 and the Asterinas `ostd` crate).

**Pros.** Keeps drivers in safe Rust; bounds-checked offsets; no references to device memory, so no aliasing UB; matches Asterinas's driver model.
**Cons.** Per-access bounds checks cost; the wrapper must be `!Sync` or carefully synchronised; DMA coherence still needs explicit cache operations.

#### Option D3-iii — Untyped `UFrame`/`USegment` with POD-only read-write interface

An untyped frame/segment permits copying POD values in and out but never forming Rust references into the region; it is the only way to expose memory that devices or users can mutate (Asterinas's untyped memory; project spec §3.4).

**Pros.** Sound for MMIO, DMA, and user-shared pages; the POD restriction is exactly the right one (a `u32` copy cannot create a dangling reference); composes with `IoMem` (an `IoMem` is a typed view over untyped memory).
**Cons.** Copy-in/copy-out costs for bulk data; drivers must be written against a narrower API; POD-only excludes structured data unless serialised.

#### Option D3-iv — `UnsafeCell` + documentation

**Pros.** Minimal abstraction.
**Cons.** Does not compose; every user must uphold the same invariants; effectively unsafe.

#### D.3 comparison table

| Option | Safe Rust possible | Aliasing soundness | Bulk performance | Fit |
|---|---|---|---|---|
| Raw pointers | No | No | Best | Rejected |
| `IoMem`/`IoPort` | Yes | Yes (no refs) | Good | **For MMIO** |
| **`UFrame`/`USegment` POD** | **Yes** | **Yes** | **Copy cost** | **For DMA/user** |
| `UnsafeCell` | No | No | Best | Rejected |

**Recommendation D.3.** **`IoMem`/`IoPort` for MMIO/PIO, `UFrame`/`USegment` for DMA and user-shared memory**, with bulk copy helpers (`copy_from_untyped`, `copy_to_untyped`) that use checked slices internally. This is the only combination that keeps drivers in safe Rust while remaining sound against device and user mutation.

**Open questions and risks.**

- Cache coherency: on MIPS, DMA buffers need explicit writeback/invalidate around transfers; the untyped API must pair with architecture-specific cache ops (Section I).
- `IoMem` on big-endian MIPS: device registers may be little-endian or big-endian; the accessor must encode endianness explicitly, not assume host order.
- User-mapped pages: `UFrame` must not allow the kernel to form references that a user can invalidate; the POD rule covers this but must be enforced by the API, not convention.

---

### D.4 Decision: sensitive vs insensitive memory classification

**Decision question.** Should the kernel classify memory as sensitive (kernel code, stacks, page tables, core MMIO) vs insensitive (user pages, DMA buffers), and how is the classification enforced?

**Pros of classification.**

- It is the basis of the framekernel invariants (Inv. 4–7): sensitive memory may not be exposed to users or devices; only insensitive memory may be DMA-mapped or user-mapped.
- It gives the framework a single predicate to check at every mapping/DMA call, which is simpler than ad hoc rules.
- It matches Asterinas's design and the project spec.

**Cons.**

- Classification must be complete: any unclassified page is a hole. Boot-time firmware tables and hot-plugged devices complicate it.
- Some memory changes class over time (a page freed from the kernel heap and returned to the allocator); the transition rules must be explicit.
- On MIPS with a software TLB, the classification must be enforced at map time, not just at allocation, or a stale TLB entry can bypass it.

**Options.**

- **D4-i — Static classification at boot**: the firmware memory map plus linker symbols define sensitive regions; everything else is insensitive.
- **D4-ii — Per-frame flag with transition rules**: each `Frame` carries a `sensitive` bit; the framework enforces transitions (only the framework can set/clear).
- **D4-iii — Capability-typed memory**: sensitivity is a property of the capability used to map, not the frame.

**Recommendation D.4.** **Per-frame flag with framework-only transitions (D4-ii), seeded from a static boot map (D4-i).** Enforce at map/DMA time and flush TLB entries on transition. Add an assertion that no sensitive frame is ever reachable from a user VMAR or a DMA mapping.

**Open questions and risks.**

- The Indy's physical memory map and device windows must be captured exactly; `hinv` and the PROM's memory descriptor are the sources.
- HPC3 and other device registers live in the 0xBF000000 region; the classification must mark them sensitive and route access through `IoMem`.
- A frame that is sensitive and then freed must be scrubbed before reclassification.

---

### D.5 Decision: VMAR/VMO as native memory objects

**Decision question.** Should native memory be modelled as VMARs (address-region allocators) and VMOs (memory objects) as in Zircon, or as a flat `mmap`-style interface?

**Option D5-i — VMAR/VMO (recommended).**

- A **VMO** is a resizable, shareable memory object (anonymous, physical, or paged-backed). Handles to VMOs can be duplicated, passed over channels, and mapped read-only/read-write/execute.
- A **VMAR** is a node in a process's address-space tree that reserves a range and can contain mappings or child VMARs.

**Pros.** Clean separation of physical memory (VMO) from address layout (VMAR); sharing is explicit; `fork` COW is a VMO clone; `mmap` maps a VMO; DMA needs a VMO with a physical backing; the model is documented by Fuchsia (https://fuchsia.dev/fuchsia/src/concepts/objects) and matches the project's ADR-0011 list.
**Cons.** More objects to implement; the VMAR tree must be kept consistent under `mmap`/`munmap`/`mprotect`; legacy `mmap` semantics (fixed addresses, `MAP_ANON`, `MAP_SHARED`) must be translated.

**Option D5-ii — Flat `mmap`-style regions.**

**Pros.** Direct mapping to the legacy ABI; simpler.
**Cons.** No clean sharing/delegation; COW and DMA are special cases; the native model becomes indistinguishable from the compat model.

**Recommendation D.5.** **VMAR/VMO**, with the compat container translating `mmap`/`brk`/`shmat` onto them. Keep the VMAR tree simple (a sorted range map with a free-list allocator) and make the VMO the unit of sharing.

**Open questions and risks.**

- `MAP_FIXED` and IRIX's `mmap` flags must be honoured exactly; the VMAR allocator must support fixed placement.
- Execute permission and W^X: legacy binaries may expect executable data; the container needs a compatibility flag, but native code should be W^X.
- VMO resizing and `mremap` semantics.

---

### D.6 Decision: TLB and cache management on MIPS III

**Decision question.** How should the kernel manage the software-managed TLB and the R4000-class caches, given that these dominate memory-system performance and correctness?

**Background.** The R4000/R4400 has a small, software-refilled TLB (48 entries on R4000-class parts) and separate instruction/data caches with write-back behaviour. DMA is not coherent: the CPU must write back/invalidate cache lines around device transfers. The kernel must implement TLB refill in the exception vector, not rely on hardware page-table walks.

**Options.**

- **D6-i — Direct-mapped kernel region + TLB for user/VMAP**: map physical memory in a kseg-style direct region (MIPS's `kseg0`/`kseg1`), and use TLB only for user mappings and vmalloc-style windows. Pros: minimal TLB pressure, simple; cons: limits kernel virtual address space and complicates >512 MiB mappings.
- **D6-ii — Fully TLB-mapped kernel**: every kernel access goes through the TLB. Pros: uniform; cons: TLB pressure and refill cost on every kernel access — bad on 48 entries.
- **D6-iii — Hybrid**: direct map for RAM, TLB for MMIO windows and user; cache ops explicit at DMA boundaries.

**Recommendation D6.** **Hybrid (D6-iii)**: use the MIPS unmapped/direct segments for kernel RAM (as IRIX and Linux/MIPS do), map MMIO via TLB-backed `IoMem`, and use TLB for all user mappings. Implement TLB refill in the exception vector with a small, audited routine; treat TLB shootdown as a cross-CPU IPI when SMP exists.

**Open questions and risks.**

- The exact kseg layout and whether the Indy's memory map allows a full direct map of RAM.
- Cache aliasing: R4000 caches are virtually indexed; user mappings with conflicting virtual addresses can alias. The kernel must either use page-coloured allocation or enforce non-conflicting mappings.
- DMA cache maintenance: `cache` instruction variants differ across R4000/R4400/R4600; the framework needs a per-CPU implementation.
- TLB refill on a fault must not allocate or take locks in interrupt context.

---

### D.7 Section D summary

| Decision | Recommended | Why |
|---|---|---|
| Core abstraction | Refcounted `Frame`/`Segment` + `MappedPages` ownership | Sound references + correct sharing |
| Metadata/invariants | Per-frame metadata, type-state + runtime checks, Inv. 1–10 | Enforceable, auditable safety |
| Externally-modifiable memory | `IoMem`/`IoPort` + `UFrame`/`USegment` POD | Safe drivers, sound against devices/users |
| Sensitivity | Per-frame flag, framework-only transitions, map-time enforcement | Basis of Inv. 4–7 |
| Native memory objects | VMAR/VMO | Clean sharing and delegation |
| TLB/cache | Hybrid direct map + TLB; explicit cache ops | Performance and correctness on R4000-class CPUs |

---

## E. Privilege separation and the TCB

### E.1 Decision: how is the kernel split between privileged framework and de-privileged services?

**Decision question.** Should all kernel code be equally trusted (Theseus's pure model, everything in Ring 0 with `unsafe` permitted wherever needed), or should a small privileged framework hold all `unsafe` while every service — compat container, drivers, filesystems, networking — is safe Rust?

#### Option E1 — Flat trust: all kernel code may use `unsafe`

**Pros.** No artificial API constraints; the shortest path to working code; matches Theseus's early model and most C kernels.
**Cons.** The TCB is the whole kernel. Every driver and every compat quirk can corrupt memory. The Asterinas paper measures this cost: existing Rust OSes have 44–66% of their code in the memory-safety TCB, versus ~14% for a framekernel (arXiv:2506.03876). For IRIX 7, whose compat container is large and security-sensitive, flat trust is the worst option.

#### Option E1b — Framekernel split: privileged framework (TCB) vs safe services

The framework owns CPU mode control, sensitive memory, page tables, interrupt controller, MMIO/PIO labelling, frame/slab allocation, and trap entry/exit. It exposes safe APIs. Everything else is `#![forbid(unsafe_code)]`.

**Pros.**

- Smallest published TCB for a general-purpose Rust OS (~14.0%, Asterinas).
- The boundary is compiler-enforced per crate, so it cannot silently erode.
- Services can be developed, reviewed, and fuzzed independently of the framework.
- Matches the project's ADR-0010 ("deep crates with language-level isolation") and the spec's two-tier table.

**Cons.**

- The framework API must be expressive enough for every service; gaps force either `unsafe` leakage or API redesign.
- Framework bugs are catastrophic (they are the TCB), so the framework needs the heaviest review, testing, and ideally verification.
- MIPS-specific framework work (TLB, cache, exception vectors) is substantial.

#### Option E1c — Microkernel-style trust boundary (kernel vs userspace servers)

**Pros.** Strongest isolation (separate address spaces).
**Cons.** Rejected in Section A for performance and complexity; the compat container must be in-kernel.

#### E.1 comparison table

| Option | TCB size | Compiler enforcement | Service ergonomics | Fit |
|---|---|---|---|---|
| Flat trust | Whole kernel | None | Best | Rejected |
| **Framekernel split** | **~14% (published)** | **Per-crate `forbid(unsafe_code)`** | **Good if APIs are right** | **Best** |
| Microkernel boundary | Small kernel | Address spaces | Poor (IPC) | Rejected |

**Recommendation E.1.** **Framekernel split**, with the framework as the only crate family permitted `unsafe`, and a workspace lint that fails the build if any de-privileged crate contains `unsafe`. Make the split visible in the repository layout: `framework/` vs `services/` with different lint configurations.

**Open questions and risks.**

- The framework API is the project's most important interface; design it before writing services, and expect to revise it.
- Some services genuinely need low-level access (e.g. a driver's register layout); provide safe abstractions (`IoMem`) rather than exemptions.
- Third-party crates may contain `unsafe`; the licence/provenance gate must also record whether a dependency is in the TCB.

---

### E.2 Decision: safe policy injection for scheduler, frame allocator, and slab allocator

**Decision question.** Should policy (which task runs next, which frame is allocated, which slab serves a request) live inside the privileged framework, or be injected as traits implemented by safe services?

#### Option E2 — Built-in policies in the framework

**Pros.** Simpler call paths; no dynamic dispatch; easier to reason about worst-case latency.
**Cons.** Every policy change touches the TCB; the framework grows; real-time scheduling (IRIX identity, Section H) would be baked in and hard to vary; the Asterinas result explicitly credits policy injection for keeping the TCB small.

#### Option E2b — Injected traits (`Scheduler`/`RunQueue`, `FrameAlloc`, `Slab`/`HeapSlot`)

**Pros.** Policy lives in safe Rust outside the TCB; multiple policies (CFS-like, real-time FIFO/RR, deadline) can coexist; the framework only enforces mechanism and invariants. This is Asterinas's design (arXiv:2506.03876 §4.4) and the project spec's §3.3.
**Cons.** Trait objects or generics add indirection; a buggy policy can still cause denial of service (it cannot corrupt memory, but it can starve tasks); the framework must validate policy outputs (e.g. a returned frame must be free).

#### Option E2c — Hybrid: mechanism in framework, policy selected at boot

**Pros.** Static dispatch for the common path, dynamic choice at boot; no runtime trait-object cost if monomorphised.
**Cons.** More build configurations; the boot-time selection must be type-safe.

**Recommendation E.2.** **Injected traits, with boot-time policy selection and framework validation of policy outputs.** Keep the traits narrow (`alloc_frame`, `free_frame`, `pick_next`, `enqueue`) and make the framework check every output against its invariants (Inv. 1, 8, 9, 10). This preserves the small TCB while enabling IRIX's real-time classes.

**Open questions and risks.**

- Trait-object dispatch on MIPS: indirect calls are cheap, but the `dyn` overhead in the allocator hot path should be measured; consider generics for the frame allocator and `dyn` for the scheduler.
- A malicious/buggy scheduler can starve the system; add a watchdog or a framework-enforced time quantum.
- Policy injection must not become an escape hatch: policies receive capabilities/safe handles, never raw frames.

---

### E.3 Decision: language-based intra-kernel privilege separation — how far?

**Decision question.** How much privilege separation can the Rust type system provide inside one address space, and where must hardware or framework checks remain?

**Evidence.** Singularity demonstrated software-isolated processes (SIPs) with a verified type-safe runtime (Hunt & Larus, "Singularity: Rethinking the Software Stack", 2007; "Safe to the last instruction", PLDI 2010). Theseus demonstrates intralingual resource management. Asterinas demonstrates the framework/services split with a 14% TCB. The lesson: language isolation is real but depends on (a) a safe language, (b) a small trusted runtime, and (c) no escape hatches (FFI, `unsafe`, raw pointers) in the isolated code.

**Options.**

- **E3-i — Pure language isolation**: all code in one ring, no hardware checks. Pros: cheapest; cons: unsafe code and DMA break it.
- **E3-ii — Language isolation + framework checks at boundaries**: the framework validates every operation that crosses a trust boundary (mapping, DMA, trap return). Pros: robust; cons: some runtime cost.
- **E3-iii — Language isolation + MMU for legacy**: the hybrid from Section A.2. Pros: contains untrusted native code; cons: complexity.

**Recommendation E.3.** **E3-ii for all Rust code, plus E3-iii's MMU boundary for legacy processes.** Add a rule: no `extern "C"` function may be called from safe service code without a safe wrapper in the framework; FFI is a privilege boundary. Audit `unsafe` blocks by count and by crate, and publish the TCB percentage as a build metric.

**Open questions and risks.**

- Rust's soundness bugs (rare but real) are TCB bugs by definition; track rustc security advisories and pin the compiler.
- `unsafe` in dependencies (e.g. allocator, `abi_stable`) counts toward the TCB; record it.
- Signals and trap return are implicit control-flow transfers that can bypass type-level invariants if not routed through the framework.

---

### E.4 Decision: invariant enforcement strategy

**Decision question.** Should the framework's invariants (Inv. 1–10) be enforced by types, runtime checks, or a mix; and how are they tested?

**Options.**

- **E4-i — Types only**: encode states in types (`FreeFrame`, `MappedFrame`), making illegal transitions unrepresentable. Pros: zero runtime cost, strong; cons: poor fit for dynamic sharing and for state that changes asynchronously (DMA, interrupts).
- **E4-ii — Runtime checks only**: assertions and safe APIs. Pros: flexible; cons: bugs are panics, and a release build may compile out `debug_assert!`.
- **E4-iii — Mix**: types for ownership/lifetime, runtime checks for sharing counts and external state; `debug_assert!` in debug, `assert!` for security-critical checks in release.

**Recommendation E.4.** **Mix (E4-iii)**, with a published invariant-to-mechanism table (as the spec already does) and tests that deliberately violate each invariant to confirm the mechanism catches it. Use `#[cfg(debug_assertions)]` for expensive checks and unconditional checks for the security-critical ones (Inv. 4–7).

**Open questions and risks.**

- Panic policy: `panic = "abort"` means an invariant violation halts the machine; that is safe but not available. Consider a recoverable path for non-security invariants.
- Invariant testing needs fault injection (e.g. a test frame allocator that returns in-use frames) to prove the checks work.

---

### E.5 Decision: stack usage analysis and guard pages

**Decision question.** How should the kernel prevent stack overflow, given no paging-based guard on a direct-mapped kernel stack and a small RAM budget?

**Options.**

- **E5-i — Compile-time stack usage analysis**: `-Z emit-stack-sizes` plus a tool such as `cargo-call-stack` (https://github.com/japaric/cargo-call-stack) to compute worst-case stack per call graph. Pros: catches recursion and large frames before deployment; cons: indirect calls and asm break the analysis; GCC backend support is uncertain.
- **E5-ii — Guard pages**: an unmapped page below each kernel stack; overflow traps. Pros: simple, robust; cons: wastes a page per stack (acceptable), and on a direct-mapped kseg the guard must be a TLB-mapped hole.
- **E5-iii — Canaries**: a magic value checked on context switch. Pros: cheap; cons: detects after the fact, and a large overflow can skip the canary.
- **E5-iv — Fixed-size stacks with static analysis + runtime watermark**: measure high-water marks and size stacks accordingly. Pros: empirical; cons: workload-dependent.

**Recommendation E.5.** **Guard pages (E5-ii) for every kernel and user stack, plus compile-time analysis (E5-i) in CI where the toolchain supports it, plus canaries (E5-iii) as a cheap backstop.** Note that `rust-irixlibstd` found IRIX stack-overflow detection needs care: IRIX has no pthread stack getters, `sigaltstack` wants the top of the stack, and reports `ss_flags = 0` rather than `SS_DISABLE` (https://github.com/danifunker/rust-irixlibstd). Those quirks matter for user-stack overflow handling in the compat layer.

**Open questions and risks.**

- `rustc_codegen_gcc` may not emit stack-size metadata; the LLVM path might be needed for this analysis even if GCC is primary.
- Interrupt handlers run on the current stack (or a separate interrupt stack); the latter is safer and should be preferred.
- User stacks need guard pages too; the compat layer must emulate IRIX's `SIGSEGV`-on-overflow behaviour.

---

### E.6 Decision: verification and testing of the TCB

**Decision question.** What assurance techniques should be applied to the privileged framework?

**Options.**

- **E6-i — Code review + `unsafe` audit only**: cheapest; weak.
- **E6-ii — Miri on framework unit tests**: detects UB in `unsafe` code under the interpreter (https://github.com/rust-lang/miri); limited by FFI/asm and by MIPS-specific code.
- **E6-iii — Kani model checking**: proves properties of Rust functions (https://github.com/model-checking/kani); good for allocator/refcount invariants; struggles with asm and large code.
- **E6-iv — Fuzzing**: syscall decoder, ELF/ECOFF loader, filesystem parsers; high value per effort.
- **E6-v — Formal verification**: seL4-style refinement proofs; far beyond the project's resources, but a long-term aspiration.

**Recommendation E.6.** **E6-ii + E6-iii + E6-iv as the working set**, with E6-v explicitly out of scope. Prioritise fuzzing the compat container (untrusted input) and Miri/Kani on the frame/slab allocator and refcount logic. Publish the TCB crate list and its `unsafe` block count per release.

**Open questions and risks.**

- Miri cannot execute MIPS asm or MMIO; framework code that touches hardware must be isolated behind thin, manually reviewed shims.
- Kani's performance on large functions; keep verified functions small.
- Fuzzing the loader needs a corpus of real IRIX binaries; licence constraints on redistribution must be respected (use locally, not in CI artefacts).

---

### E.7 Section E summary

| Decision | Recommended | Why |
|---|---|---|
| Trust split | Framekernel: framework holds `unsafe`, services `forbid(unsafe_code)` | Smallest published TCB; compiler-enforced |
| Policy | Injected traits, boot-time selection, framework validation | Keeps scheduler/allocator policy out of the TCB |
| Language isolation | Framework checks at boundaries + MMU for legacy | Sound where types suffice, hardware where they do not |
| Invariants | Types + runtime checks; unconditional for security-critical | Practical and auditable |
| Stacks | Guard pages + compile-time analysis + canaries | Cheap, robust on small-RAM MIPS |
| Assurance | Miri, Kani, fuzzing; no formal proof | Best value for resources |

---

## F. Boot and bring-up, including the two-phase ROM handoff

### F.1 Decision: what is the boot path?

**Decision question.** How should IRIX 7 get from power-on to a running kernel: the factory SGI PROM (ARCS) loading an o32 ECOFF image, a custom bootloader, Multiboot2, UEFI, or the proposed two-phase "Physical Rust ROM" that fetches the kernel over the network?

#### Option F1 — Factory SGI PROM (ARCS) + o32 ECOFF bootstrap

The PROM initialises the machine, runs its power-on tests, and loads an o32 ECOFF image from disk, tape, or network (TFTP/BOOTP); the image sets up the kernel and jumps to it.

**Pros.**

- Zero bootloader to write; the firmware is already in the ROM and is the machine's authority on memory, devices, and console. This is ADR-0013's choice and the tracer bullet's proven path (ADR-0016).
- ARCS provides a documented set of callbacks (console output, memory descriptors, device tree/hinv, disk and network I/O) that the bootstrap can use before it has drivers.
- Works on real hardware and on the `irix7/iris` emulator, which implements the same PROM interface.
- ECOFF/o32 is exactly what the PROM understands; the constraint is well understood (`rust-irixlibstd` confirms o32 is MIPS II and that LLVM cannot build o32 against MIPS III).

**Cons.**

- The PROM's boot environment is archaic: ECOFF only, o32 only, limited loaders, and environment variables rather than a modern boot protocol.
- The PROM's device support is limited (SCSI, Ethernet via TFTP); no HTTP/TLS.
- Boot is slow, and iterating on a kernel image means rebuilding an ECOFF and restarting the emulator.
- The PROM may leave devices in unknown states; the bootstrap must reinitialise.

#### Option F1b — Custom bootloader (e.g. a small Rust loader on disk)

**Pros.** Full control; can load ELF/n32 directly; can implement a modern protocol; can chain-load multiple kernels.
**Cons.** Must itself be loaded by the PROM (so it is ECOFF/o32 anyway); duplicates firmware work; another component to debug; no clear benefit for a single-machine OS.

#### Option F1c — Multiboot2 / GRUB

**Pros.** Standard, well-documented (https://www.gnu.org/software/grub/manual/multiboot2/), good tooling, used by many hobby OSes (e.g. Theseus supports GRUB and Limine; https://github.com/theseus-os/Theseus).
**Cons.** x86-centric; no MIPS/ARCS support. Irrelevant to the Indy.

#### Option F1d — UEFI

**Pros.** Modern, standard (https://uefi.org/specifications), good Rust support via `uefi-rs` (https://github.com/rust-osdev/uefi-rs).
**Cons.** The Indy has no UEFI and never will; adding a UEFI layer would be emulation on top of the PROM. Irrelevant.

#### Option F1e — Two-phase Physical Rust ROM (proposed)

Phase 1 is a `no_std` Rust ROM (512 KiB–1 MiB flash) that initialises the hardware and the Seeq 80C03 Ethernet, runs a dual-stack IPv4/IPv6 smoltcp stack, fetches a kernel over HTTP/HTTPS into a verified RAM block (e.g. `0x80040000`), and jumps to it. Phase 2 is the kernel, which inherits networking state and mounts a GitHub LFS object as a virtual block device via HTTP Range requests.

**Pros.**

- Removes dependence on the PROM's ancient disk/network loaders; the ROM is a known, auditable Rust program.
- Network boot is operationally convenient for an emulator and for a machine whose SCSI disks are dead or missing.
- The ROM can verify the kernel image (SHA-256/signature) before jumping, which the PROM cannot.
- It is a genuinely interesting bring-up vehicle and exercises the same drivers/networking code the kernel will need, in the smallest possible configuration.
- A Rust ROM is a strong demonstration of the project's thesis (safe Rust at the lowest level).

**Cons.**

- **TLS 1.3 on bare metal with no clock** is a hard chicken-and-egg problem (see F.4).
- **Size**: smoltcp + TLS + HTTP + Seeq driver + MIPS init in 512 KiB–1 MiB is tight; TLS alone can be 50–150 KiB of code and tens of KiB of RAM.
- **The PROM still boots the ROM**: the ROM must itself be an ECOFF/o32 image the PROM accepts, so the two-phase design does not remove the ARCS constraint; it adds a second stage.
- **Networking from a ROM** requires DHCP/ARP/IPv6 ND, DNS, and a TCP stack before any OS exists — a lot of failure modes with no filesystem or shell to debug from.
- **Trust**: fetching a kernel over the network moves the root of trust to TLS certificate validation, which needs a clock and a CA store in ROM.
- **The Indy's Seeq 80C03 is a 10 Mbit/s controller**; downloading a multi-megabyte kernel over TFTP/HTTP is slow but tolerable.
- The ROM is a second kernel to maintain; code sharing with the real kernel is limited by `no_std` and size.

#### F.1 comparison table

| Option | Firmware compatibility | Network boot | Kernel verification | Effort | Risk | Fit |
|---|---|---|---|---|---|---|
| **Factory PROM + ECOFF** | **Native** | TFTP only | None (trust PROM) | Low | Low | **Baseline, always** |
| Custom bootloader | Needs ECOFF shim | Custom | Possible | Medium | Medium | Optional |
| Multiboot2/GRUB | None (x86) | N/A | N/A | N/A | N/A | Irrelevant |
| UEFI | None (Indy) | N/A | N/A | N/A | N/A | Irrelevant |
| **Two-phase Rust ROM** | **ROM is ECOFF, then Rust** | **HTTP/HTTPS** | **SHA-256/signature** | **High** | **High (TLS, size)** | **Valuable experiment, not baseline** |

**Recommendation F.1.** **Keep the factory PROM + o32 ECOFF bootstrap as the baseline and the tracer bullet's path** (ADR-0013/ADR-0016). Pursue the two-phase Rust ROM as a **separate, optional bring-up vehicle** for emulator-based development and for machines without working disks — not as the primary boot path until TLS and size are solved. Design the ROM so it shares its Seeq driver and smoltcp configuration with the kernel, and so the kernel can be booted either way. This preserves the proven path while capturing the ROM's benefits.

**Open questions and risks.**

- Is the ROM's flash actually writable/reflashable on an Indy, or is "ROM" a misnomer for a boot image on disk? The answer determines whether the two-phase design is deployable or emulator-only.
- The PROM's ECOFF loader expects a specific image layout; the ROM must satisfy it before it can do anything modern.
- The ROM's network stack must coexist with the PROM's own network state (the PROM may have already configured the Seeq); reinitialising is safer.

---

### F.2 Decision: the Phase 1 ROM's structure and size budget

**Decision question.** How should the Phase 1 ROM be structured to fit in 512 KiB–1 MiB while providing Ethernet, dual-stack IP, TLS, HTTP, verification, and a jump?

**Budget sketch (to be measured, not assumed).**

| Component | Code (KiB) | RAM (KiB) | Notes |
|---|---|---|---|
| MIPS init, exception vectors, cache/TLB setup | 8–16 | 4–8 | Assembly + small Rust |
| Seeq 80C03 driver + HPC3 DMA | 8–16 | 8–16 | Descriptor rings, cache maintenance |
| smoltcp (IPv4+IPv6, TCP, DHCP/ND, DNS) | 60–120 | 32–64 | Feature-gated; no sockets unused |
| TLS 1.3 (rustls or embedded-tls) | 80–200 | 32–64 | Handshake buffers, CA store |
| HTTP/1.1 client + Range | 8–16 | 8–16 | Minimal request/response parser |
| SHA-256 verification | 4–8 | 1–2 | Streaming |
| Panic/log/console | 4–8 | 2–4 | Serial + PROM console |
| **Total** | **~170–390** | **~90–180** | Fits 512 KiB with headroom if TLS is lean |

**Options.**

- **F2-i — Monolithic ROM image**: everything linked, one image. Pros: simplest, best LTO; cons: no modularity, hard to test pieces.
- **F2-ii — Feature-gated single image**: one crate graph with Cargo features (`tls-rustls`, `tls-embedded`, `ipv6`, `dhcp`), producing different ROMs. Pros: testable on host, size-tunable; cons: feature-combination complexity.
- **F2-iii — Staged ROM**: a tiny first stage initialises serial and memory, then loads a larger second stage over the wire. Pros: smallest first stage; cons: two network stages, more failure modes.

**Recommendation F.2.** **Feature-gated single image (F2-ii)**, built as an o32/MIPS II ECOFF for the PROM, with a host-testable core (smoltcp's `phy` abstraction allows a tap/loopback backend on Linux). Keep TLS optional at build time so a TFTP-only diagnostic ROM exists.

**Open questions and risks.**

- Rust `core` + `alloc` size on MIPS: `-Z build-std` with `panic_immediate_abort` and `opt-level = "z"`; measure.
- smoltcp's buffer sizes are compile-time constants; the ROM must size them for 1500-byte MTU and one TCP connection.
- The Seeq 80C03 driver must handle the HPC3's DMA engine and the Indy's cache coherency; this is the highest-risk driver in the ROM.

---

### F.3 Decision: Phase 1 network stack and packet path

**Decision question.** Should the ROM use smoltcp, a hand-rolled stack, lwIP, or the PROM's network callbacks?

**Options.**

- **F3-i — smoltcp**: a standalone, event-driven TCP/IP stack designed for bare-metal and real-time systems; no heap required; IPv4 and IPv6; TCP with retransmission, window scaling, CUBIC/Reno; DHCP and DNS sockets (https://github.com/smoltcp-rs/smoltcp). **Pros:** exactly the target environment, 0BSD licence (GPL-compatible), documented feature set, used widely in embedded Rust. **Cons:** no TLS (separate), no HTTP (separate), buffer sizing is compile-time, and its `no_std` + `alloc` combinations must be checked.
- **F3-ii — lwIP**: mature C stack, small footprint, used in firmware. **Pros:** battle-tested, TLS via mbedTLS/BearSSL. **Cons:** C, `unsafe` FFI, licence (BSD-3), and it contradicts the Rust-ROM premise.
- **F3-iii — Hand-rolled stack**: minimal TCP/IP for one HTTP GET. **Pros:** smallest, fully controlled. **Cons:** TCP correctness (retransmission, congestion, reassembly) is a deep rabbit hole; not worth it.
- **F3-iv — PROM network callbacks**: use ARCS's network I/O. **Pros:** no driver. **Cons:** the PROM's network stack is ancient and TFTP-oriented; no IPv6/TLS; not a path to the kernel's networking.

**Recommendation F.3.** **smoltcp** for both Phase 1 and the kernel's early networking. Its design goals (bare metal, no heap required, IPv4+IPv6) match the ROM exactly, and 0BSD clears the licence gate. Drive it from the Seeq receive path: the driver copies frames from the HPC3 DMA ring into smoltcp's `RxToken` buffers; the transmit path copies from `TxToken` into descriptors. Keep the driver and the stack behind a small `phy` implementation so the same stack runs against a tap device on the host for tests.

**Open questions and risks.**

- smoltcp's `alloc` feature vs a fully static buffer configuration; the ROM should avoid a heap if possible.
- IPv6 neighbour discovery and DHCPv6 vs static configuration; the ROM needs a policy.
- DNS over IPv6 and A/AAAA fallback.
- The Seeq's 10 Mbit/s line rate and small receive ring: TCP window scaling and buffer sizing must match.

---

### F.4 Decision: TLS 1.3 with no trustworthy clock

**Decision question.** How should the ROM validate GitHub's TLS certificate when the Indy's RTC battery may be dead and there is no trustworthy time source?

**The problem.** TLS certificate validation checks the certificate chain, the hostname, and the validity period (`notBefore`/`notAfter`). GitHub uses short-lived certificates (CA/Browser Forum rules cap validity at 398 days; https://cabforum.org/), so a compile-time timestamp baked into the ROM becomes invalid within months. The RTC may be dead, and network time (NTP/SNTP) is unauthenticated, so it cannot be trusted before TLS. This is a genuine chicken-and-egg problem.

**Options.**

- **F4-i — Read the RTC; if valid, use it.** Pros: correct when the battery is alive. Cons: dead battery is the stated risk; an RTC can also be wrong (timezone/epoch), and a wrong time fails validation.
- **F4-ii — Compile-time timestamp.** Pros: trivial. Cons: expires; requires rebuilding the ROM every few months; a stale ROM cannot boot. Only acceptable for a lab/emulator.
- **F4-iii — Override certificate expiry (validate chain + hostname, ignore `notAfter`).** Pros: works with a dead clock; still authenticates the server's key and name. Cons: defeats revocation/expiry semantics; a compromised or retired key remains trusted; not acceptable as a general design, but pragmatic for a hobbyist machine on a trusted network.
- **F4-iv — Pin GitHub's public key/SPKI instead of validating a chain.** Pros: no clock needed; strong against MITM if the pin is current. Cons: key rotation breaks it; pins must be updated with the ROM; still needs a trust anchor update path.
- **F4-v — Fetch time over HTTPS from a trusted source first (e.g. `Date` header), then validate the kernel download.** Pros: bootstraps a clock from the TLS handshake itself. Cons: the first connection still needs validation; if you accept an unvalidated `Date`, an attacker controls time.
- **F4-vi — Use a network time protocol with authentication (NTS).** Pros: authenticated time. Cons: NTS itself needs TLS; circular.
- **F4-vii — Hardware secure element / trusted time source.** Pros: proper root of trust. Cons: the Indy has none; adding hardware is out of scope.
- **F4-viii — Don't use HTTPS in the ROM: use TFTP/HTTP on a trusted LAN, verify the kernel by signature.** Pros: no TLS chicken-and-egg; the kernel image is signed and verified with an embedded public key, which is a cleaner root of trust than a certificate chain. Cons: the download is visible/tamperable in transit (signature catches tampering, not confidentiality); GitHub requires HTTPS, so this only works with a local mirror.

**Comparison table.**

| Option | Works with dead RTC | Survives cert rotation | Security | Operational cost | Fit |
|---|---|---|---|---|---|
| RTC | No | Yes | Good if clock right | Low | Conditional |
| Compile-time time | Yes (until expiry) | No | Good until expiry | Rebuild often | Lab only |
| Ignore expiry | Yes | Yes | Weak (no revocation/expiry) | None | Pragmatic fallback |
| Pin SPKI | Yes | No (pin update) | Strong until rotation | Rebuild on rotation | Viable |
| Date-header bootstrap | Yes | Yes | Circular/weak first hop | Low | Not sound |
| NTS | Yes | Yes | Strong | High | Circular |
| Secure element | Yes | Yes | Strongest | Hardware | Out of scope |
| **TFTP/HTTP + signed kernel** | **Yes** | **Yes** | **Good (signature), no confidentiality** | **Low** | **Best for ROM** |

**Recommendation F.4.** **Separate authenticity from transport.** For the ROM, prefer **HTTP/TFTP on a trusted network plus a signed kernel image verified with an embedded public key** (F4-viii); this removes the clock dependency entirely and gives a stronger root of trust than a certificate chain validated with a possibly-wrong clock. If HTTPS is required (e.g. fetching directly from GitHub), use **F4-iii (chain + hostname, expiry override) with a pinned SPKI as a second factor**, and document it as a lab-grade compromise. Never rely on a compile-time timestamp for anything but an emulator image.

For Phase 2 (the running kernel), once a clock exists (RTC or authenticated time), switch to full TLS validation with a CA store; the kernel can also re-verify the image it was booted from.

**Open questions and risks.**

- The Indy's RTC/NVRAM battery (Dallas semiconductor part) may be dead; replacing it is a hardware task, and even a live RTC can drift.
- GitHub's certificate chain and key rotation schedule; a pinned SPKI will need updates.
- TLS 1.3 implementation size: rustls (https://github.com/rustls/rustls) with `no_std` + alloc, or embedded-tls (https://github.com/drogue-iot/embedded-tls); measure both on MIPS.
- If the ROM ignores expiry, an attacker with a stolen but unexpired key can impersonate GitHub; the signature check on the kernel mitigates the consequence.
- The ROM's CA store must be updatable without reflashing, or it will rot.

---

### F.5 Decision: big-endian MIPS vs little-endian network and cloud data

**Decision question.** How should the code handle the impedance mismatch between big-endian MIPS III and the little-endian conventions used by network protocols and cloud APIs?

**The facts.** MIPS III on the Indy is big-endian. Network byte order is big-endian by definition (RFC 791/8200), so IP/TCP/UDP headers are *native* on the Indy — a rare advantage. But many file formats and cloud APIs are little-endian: Git LFS pointer metadata is text (endian-free), HTTP is text, but the *contents* of LFS objects (disk images, ELF files) may be little-endian, and any binary protocol (e.g. some S3 XML? XML is text) needs explicit conversion. Rust's `to_be`/`from_be_bytes` and `to_le`/`from_le_bytes` make the intent explicit.

**Options.**

- **F5-i — Explicit conversion at every boundary**: use `u32::from_be_bytes` for network headers and `from_le_bytes` for little-endian file formats; never `transmute`. Pros: correct, auditable, no assumptions; cons: verbose, easy to miss a field.
- **F5-ii — Typed wrappers (`Be32`, `Le32`)**: newtypes that only convert explicitly. Pros: type system catches mistakes; cons: boilerplate.
- **F5-iii — Assume native order**: works for network headers on big-endian, breaks for little-endian payloads. Pros: fast; cons: silently wrong.
- **F5-iv — Byte-swap the whole image at load**: convert a little-endian disk image to big-endian. Pros: one-time cost; cons: only valid for fixed-width data, breaks text/structures with mixed widths.

**Recommendation F5.** **Explicit typed conversions at boundaries (F5-i + F5-ii)**, with a lint/test that scans for `transmute` and raw pointer casts in protocol code. Treat the network headers as big-endian native (no swap) but still use the typed API for clarity. For ELF and disk-image parsing, parse field by field with `from_le_bytes`/`from_be_bytes` as the format dictates. Add a big-endian CI job (the MIPS target is big-endian by construction) and host-side tests with known vectors.

**Open questions and risks.**

- The Seeq DMA descriptors and HPC3 registers may have their own endianness; document each.
- TLS record headers are big-endian; the TLS library handles this, but any hand-rolled record layer must not assume.
- Git LFS pointer files are ASCII text; SHA-256 is bytes; the size is decimal text — no endian issue, but parsing must be strict.
- Any binary format embedded in the LFS object (e.g. a FAT32 image) has its own endianness rules; the FS driver must follow them.

---

### F.6 Decision: the in-image boot sequence

**Decision question.** What should happen between the PROM handing control to the image and the kernel's main loop — and in what order?

**Recommended sequence (adapted from Theseus's `nano_core`/`captain` and the project spec §6.1).**

1. **Assembly entry (`_start_asm`)**: set up a stack in the image's `.bss`, clear `.bss`, save the PROM's argument registers (memory descriptor, device tree/hinv pointer, console callbacks), disable interrupts, set the status register to a known state, and jump to Rust.
2. **Minimal page table / TLB setup**: identity-map the kernel image and a direct-map window for RAM; install exception vectors (TLB refill, general exception, interrupt); enable the FPU only if needed; set the cache configuration.
3. **Early console**: polled serial (the Indy's `z85c30` SCC or the PROM's `putchar` callback) so the first marker prints before any driver exists. This is the tracer bullet's "marker print".
4. **Rust `_start`**: `extern "C" fn _start() -> !`, `#![no_std]`, `#![no_main]`, `#[panic_handler]`, `panic = "abort"`. Initialise logging, parse the boot information (memory map, hinv, console), and print a boot banner.
5. **`nano_core`**: the minimal bootstrap — load the first cell namespace (or the statically linked image), initialise the frame allocator from the memory descriptor, create the first address space, remap the kernel into it, install the real trap handlers.
6. **`captain`**: the first task on the boot CPU; it initialises the platform (HPC3, interrupts, timers, Seeq, SCSI), starts the device drivers and services, creates the first user process (the compat container's init, or a native init), and enters the idle loop. On SMP, secondary CPUs start as "satellites" that wait for work.

**Options for the boundary between assembly and Rust.**

- **F6-i — Minimal assembly, maximal Rust**: assembly only sets the stack and jumps; Rust does page tables and vectors. Pros: more code in safe Rust; cons: exception vectors and TLB refill must be assembly (they run before Rust state exists).
- **F6-ii — Assembly does vectors/TLB, Rust does the rest**: the recommended split. Pros: the unavoidable assembly is small and auditable; cons: two languages at the boundary.
- **F6-iii — All assembly until `main`**: traditional; cons: unmaintainable and unsafe by construction.

**Recommendation F6.** **F6-ii**: keep assembly to the entry stub, exception vector stubs, and TLB refill; write everything else in Rust. Use a linker script that places `.text` at the PROM's load address, aligns the stack, and exports symbols for the memory map. Print the marker as early as possible — the tracer bullet's success criterion is a visible string on serial/PROM console.

**Open questions and risks.**

- The PROM's entry state (registers, cache mode, endianness) must be captured exactly; the Indy's ARCS passes a memory descriptor and a device-tree-like structure.
- Exception vector placement on MIPS: the general exception vector is at a fixed address (`0x80000180` in kseg0), and the TLB refill vector at `0x80000000`; the linker script must not collide with them.
- The kernel's load address (e.g. `0x80040000` as the ROM plans) must avoid the PROM's reserved memory and the exception vectors.
- Stack alignment: MIPS o32/n32 require 8-byte (o32) or 16-byte (n32) stack alignment at calls; the entry stub must set it.

---

### F.7 Decision: `no_std` kernel mechanics

**Decision question.** Which `no_std`/`no_main` mechanics should the kernel and ROM use, and how should panics, allocation, and entry points be handled?

**Recommended mechanics.**

- `#![no_std]` and `#![no_main]` for the kernel and ROM; `core` always, `alloc` only where a heap exists (the kernel has one; the ROM should avoid it).
- `#[panic_handler]` that prints a message and halts (or reboots via the PROM); `panic = "abort"` in both `[profile.dev]` and `[profile.release]` because there is no unwinder on bare metal.
- `extern "C" fn _start() -> !` with `#[unsafe(no_mangle)]`; the diverging return type tells the compiler the entry never returns.
- A linker script (`kernel.ld`) defining the memory layout, sections, and symbols (`__bss_start`, `__bss_end`, `__stack_top`).
- `-Z build-std=core,alloc,compiler_builtins` with the custom target JSON; `build-std-features=compiler-builtins-mem` if `memcpy`/`memset` are needed.
- `panic_immediate_abort` for the ROM to cut size.

**Options.**

- **F7-i — `no_std` + `alloc` with a kernel heap**: needed for `Vec`, `String`, `Box` in services; the heap is a slab/buddy allocator over frames.
- **F7-ii — `no_std` without `alloc`**: only `core`; smallest, but painful for services (fixed arrays, `heapless` collections).
- **F7-iii — `std` with a custom PAL**: not possible on bare metal without a full libc/runtime; rejected.

**Recommendation F7.** **`no_std` + `alloc` for the kernel; `no_std` without `alloc` for the ROM** where possible (smoltcp can run without a heap). Use `panic = "abort"` everywhere. Keep the panic handler minimal and re-entrancy-safe (no allocation, no locks).

**Open questions and risks.**

- `compiler_builtins` on MIPS III: soft-float is selected in the target JSON (`+soft-float`), so compiler-rt builtins for 64-bit division and floating point must be present; `rustc_codegen_gcc` links `libgcc` instead.
- The global allocator must be initialised before any `alloc` use; the `nano_core` must set it up from the frame allocator.
- Panic in an interrupt handler must not deadlock; use a separate panic path.

---

### F.8 Decision: early console

**Decision question.** What should the first console be: the PROM's `putchar` callback, a polled SCC driver, or the framebuffer?

**Options.**

- **F8-i — PROM console callbacks**: ARCS provides `putchar`/`getchar`; zero driver work, works before any initialisation. Pros: immediate, reliable; cons: slow, and the PROM may not have a console configured; not available in the ROM's own network phase if the PROM state is disturbed.
- **F8-ii — Polled SCC (z85c30) driver**: direct serial output. Pros: fast, independent of the PROM; cons: needs the Indy's serial register addresses and baud setup.
- **F8-iii — Framebuffer text**: Pros: visible on a monitor; cons: needs the Newport/GR2 graphics init, far too much for early boot.
- **F8-iv — Both PROM and SCC**: use the PROM first, switch to SCC when initialised.

**Recommendation F8.** **F8-iv**: PROM console for the first lines (guaranteed), then a polled SCC driver once the hardware is mapped; keep both behind a `Console` trait so the rest of the kernel does not care. The tracer bullet's marker should go to both if possible.

**Open questions and risks.**

- The Indy's serial ports are on the HPC3 (z85c30); the register offsets and interrupt behaviour must be captured.
- The PROM's console device may be the framebuffer or serial depending on the `console` environment variable.
- Early output must not use locks that could deadlock before the scheduler exists.

---

### F.9 Decision: Phase 2 — mounting a GitHub LFS object as a virtual block device

**Decision question.** How should the running kernel turn a GitHub LFS object into a block device without downloading it whole?

**The mechanism.**

1. **Fetch the LFS pointer file** (small, text): `version https://git-lfs.github.com/spec/v1`, `oid sha256:<hex>`, `size <decimal>` (https://github.com/git-lfs/git-lfs/blob/main/docs/spec.md).
2. **Authenticate to the LFS API**: `POST /repos/{owner}/{repo}/lfs/objects/batch` with the OID and size; the response gives an `href` (a signed S3/Azure URL) and headers (https://docs.github.com/en/rest/repos/lfs; https://github.com/git-lfs/git-lfs/tree/main/docs/api).
3. **Serve 512-byte sectors via HTTP Range requests**: `GET <href>` with `Range: bytes=start-end`; S3/Azure support range reads (RFC 9110 §14). Cache sectors/blocks in RAM; prefetch sequentially.
4. **Expose a block device**: a `BlockDevice` trait with `read_sector(n) -> [u8; 512]`; the FS driver above it does not know the data is remote.

**Options.**

- **F9-i — Synchronous range reads per sector**: simplest; each 512-byte read is an HTTP request unless cached. Pros: trivial; cons: latency and request amplification (a 4 KiB FS block = 8 requests).
- **F9-ii — Block cache with read-ahead**: cache 4–64 KiB blocks, prefetch sequentially, coalesce ranges. Pros: near-sequential throughput, far fewer requests; cons: cache memory, eviction policy.
- **F9-iii — Prefetch the whole file if it fits**: Pros: simplest fast path; cons: the premise says the file does not fit.
- **F9-iv — HTTP/2 multiplexing / parallel ranges**: Pros: hides latency; cons: smoltcp is HTTP/1.1-oriented; complexity.
- **F9-v — Signed-URL expiry handling**: the LFS `href` expires; refresh via the batch API on 403. Pros: robust; cons: needs credentials and a refresh path.

**Recommendation F9.** **F9-ii (block cache + read-ahead) with F9-v (refresh on expiry)**, exposing a `BlockDevice` trait. Use 512-byte sectors at the interface (matching the prompt and traditional disks) but cache 32–64 KiB blocks internally; coalesce adjacent sector reads into one Range request. Verify the whole object's SHA-256 incrementally as blocks arrive, and fail the mount if it mismatches.

**Open questions and risks.**

- GitHub LFS API authentication: a token is needed for private repos; public repos may allow anonymous batch requests, but rate limits apply.
- S3/Azure signed URLs expire (often 1 hour); long-running mounts need refresh.
- Range request semantics: some servers return 200 with the whole object instead of 206; the client must handle both and must not buffer the whole body.
- TLS on the kernel side: the kernel inherits the ROM's TLS state or re-establishes it; either way, the clock problem returns (F.4).
- Error handling: a network drop mid-read must not corrupt the FS; retries with idempotent ranges are safe.

---

### F.10 Decision: filesystem on the LFS virtual disk — FAT32, ext4, or a custom read-only layout?

**Decision question.** Should the GitHub LFS object be formatted as FAT32 or ext4, or use a custom read-only layout?

**Options.**

- **F10-i — FAT32**: simple, widely documented (Microsoft FAT specification; FatFs implementation at http://elm-chan.org/fsw/ff/00index_e.html), read/write, tooling everywhere. **Pros:** easy to create images with `mkfs.vfat`, easy to inspect on a host, drivers exist in Rust (`fatfs` crate). **Cons:** no permissions, 4 GiB file-size limit, cluster slack, FAT chain traversal is random-access-heavy (bad over HTTP ranges), no checksums, and the FS metadata is disproportionately large for a single-file image.
- **F10-ii — ext4**: mature, journaling, permissions, extents. **Pros:** Linux-native, robust, large files. **Cons:** complex (journal, extents, checksums, 64-bit feature flags), a read-only ext4 driver is a large amount of code, and journal replay is meaningless for a read-only remote image. Overkill and risky.
- **F10-iii — Custom read-only layout**: a header (magic, version, SHA-256, block size, file table) plus contiguous extents; the block device maps logical blocks to byte ranges. **Pros:** trivial to parse, deterministic, one Range request per extent, no metadata churn, easy to verify; **cons:** no host tooling (write a generator), no general-purpose compatibility, must design for future growth.
- **F10-iv — ISO 9660 / Rock Ridge**: read-only, standard, simple, host tooling (`genisoimage`), random access by extent. **Pros:** standard, simple, widely supported; **cons:** 2 GiB/4 GiB limits (with extensions), metadata overhead, less flexible than a custom layout.
- **F10-v — SquashFS / EROFS**: read-only compressed, host tooling, good space efficiency. **Pros:** mature, compressed, random access; **cons:** decompressors (zlib/xz/zstd/lz4) in the kernel, larger code, and the compression defeats range-request locality unless block-aligned.
- **F10-vi — cpio/ustar**: streaming archives. **Pros:** trivial; **cons:** no random access without an index; unsuitable as a block device.
- **F10-vii — littlefs**: designed for flash, wear levelling, small. **Pros:** robust for flash; **cons:** not designed for block-device-over-HTTP, and its metadata model assumes local writes.

**Comparison table.**

| Option | Parser size | Random access over ranges | Host tooling | Permissions | Fit |
|---|---|---|---|---|---|
| FAT32 | Medium | Poor (FAT chain) | Excellent | No | Fallback |
| ext4 | Large | Good (extents) | Excellent | Yes | Overkill |
| **Custom RO** | **Tiny** | **Excellent (extents)** | **Write your own** | **Designable** | **Best** |
| ISO 9660 | Small | Good | Good | No (Rock Ridge helps) | Good fallback |
| SquashFS/EROFS | Medium + decompressor | Good (block-aligned) | Good | Yes | If compression matters |
| cpio/ustar | Tiny | None | Good | No | Rejected |
| littlefs | Small | Poor | Some | No | Rejected |

**Recommendation F.10.** **Custom read-only layout (F10-iii)** for the LFS virtual disk, with **ISO 9660 as the interoperability fallback** and FAT32 only if a host-side tool must produce the image. Rationale: the disk is a single immutable object fetched by range; a custom layout with extent-based files gives one Range request per file read, no FAT-chain random walks, and a tiny parser that stays outside the TCB if written in safe Rust. Include a SHA-256 of the whole image and per-extent checksums so corruption is detected at the block layer.

**Open questions and risks.**

- The image must be generated reproducibly (a small Rust tool) and verified against the LFS SHA-256.
- If the LFS object is itself a standard disk image (e.g. an ext4 image produced elsewhere), the custom layout is not applicable; support a "passthrough" mode for raw images.
- Block size: 512-byte sectors at the interface, 4–64 KiB extents internally; align extents to the HTTP range granularity.
- Read-only is a feature here, but the compat layer may expect a writable root; use an overlay (RAM-backed upper layer) if needed.

---

### F.11 Section F summary

| Decision | Recommended | Why |
|---|---|---|
| Primary boot | Factory PROM + o32 ECOFF bootstrap | Proven, native, no new firmware |
| ROM | Two-phase Rust ROM as optional vehicle | Useful for diskless/emulator bring-up; not baseline until TLS/size solved |
| ROM stack | smoltcp, feature-gated, static buffers | Bare-metal design fit, 0BSD, IPv4+IPv6 |
| TLS | Signed kernel + HTTP/TFTP for ROM; full TLS once a clock exists | Removes the dead-RTC chicken-and-egg |
| Endianness | Explicit `to_be`/`from_le_bytes` at boundaries | Correct on big-endian MIPS |
| Boot sequence | Minimal asm → vectors/TLB → Rust `_start` → `nano_core` → `captain` | Small auditable assembly, Rust everywhere else |
| Console | PROM first, then polled SCC behind a trait | Earliest possible marker |
| LFS block device | Block cache + read-ahead + Range requests + refresh | Avoids whole-file download, hides latency |
| Disk format | Custom read-only extent layout; ISO 9660 fallback | Tiny parser, one request per extent |

---

## G. Toolchain and codegen

### G.1 Decision: codegen backend — `rustc_codegen_gcc` primary, LLVM parallel, or LLVM only?

**Decision question.** Which backend should produce MIPS III n32 (and o32) code: the GCC backend via libgccjit, LLVM with custom targets, or both?

#### Option G1 — `rustc_codegen_gcc` (libgccjit) as primary

`rustc_codegen_gcc` translates MIR to GCC's GIMPLE through libgccjit, then GCC's MIPS backend emits code (https://github.com/rust-lang/rustc_codegen_gcc; https://gcc.gnu.org/onlinedocs/jit/).

**Pros.**

- GCC has first-class, decades-old support for `mips-sgi-irix6.5` and for MIPS III/IV with o32/n32/n64; it is the compiler the IRIX ecosystem actually used. The project's fork is `rust-lang/gcc` with GCC 17 trunk + libgccjit (ADR-0019), and the IRIX patch series applies (per `docs/spec/architecture.md` §2.1).
- GCC's MIPS backend handles the o32 + MIPS III combination that LLVM reportedly cannot (`-mabi=32 -march=mips3` is routine for GCC), removing the "o32 implies MIPS II" restriction on the primary path.
- The IRIX ABI details (calling conventions, GP-relative addressing, `-mno-abicalls`, small-data sections) are GCC's native territory.
- The project has already proven codegen for both custom targets with this path (per the spec's status table).

**Cons.**

- `rustc_codegen_gcc` is a second-class Rust backend: some features lag, `-Z build-std` integration is less exercised, and debugging/backtraces are less mature than LLVM.
- libgccjit is a large dependency and a build-time burden (the project notes disk-space blockers).
- GCC's codegen quality on MIPS may differ from LLVM's; benchmark.
- The backend interface is unstable and tracks rustc nightly; breakage is expected.

#### Option G2 — LLVM custom targets as primary

Use stock rustc with custom target JSON for `mips2-sgi-irix7-o32` and `mips3-sgi-irix7-n32`, built with `-Z build-std` (https://doc.rust-lang.org/rustc/targets/custom.html).

**Pros.**

- Stock rustc, no backend fork; best-supported path for `no_std`/`build-std`.
- LLVM's MIPS64 backend is well-exercised (the n32 target uses it), and `rust-irixlibstd` demonstrates n32/MIPS III building and running on real IRIX 6.5.22 (https://github.com/danifunker/rust-irixlibstd).
- Better tooling: `llvm-objdump`, LTO, sanitizers (host-side), and Rust's usual diagnostics.
- The project already has target JSON files for both ABIs (`targets/`).

**Cons.**

- **LLVM cannot compile o32 against a 64-bit ISA**: `-mcpu=mips3` with o32 crashes instruction selection (confirmed by `rust-irixlibstd`), so the LLVM o32 target is MIPS II only. For a kernel that wants MIPS III everywhere, this is a real limitation for o32 bootstrap code.
- Custom target JSONs are easy to get subtly wrong (data layout, ABI name, atomic width, relocation model); validation is manual.
- LLVM's MIPS backend is in maintenance mode relative to AArch64/x86; long-term risk.
- Big-endian MIPS III codegen bugs are less likely to be caught upstream.

#### Option G3 — Both: GCC primary, LLVM parallel (project's ADR-0018)

**Pros.**

- Cross-checks: if both backends produce working code, the ABI assumptions are validated; differences expose bugs.
- Lets each backend cover the other's gaps (GCC for o32+MIPS III, LLVM for tooling and n32).
- Keeps the project resilient to either backend's breakage.

**Cons.**

- Two toolchains to build, test, and keep in sync; double the CI and disk cost.
- Divergent codegen can mask or create target-spec bugs.
- The team's attention is split.

#### G.1 comparison table

| Option | o32 + MIPS III | n32 + MIPS III | Rust feature completeness | Build cost | Risk | Fit |
|---|---|---|---|---|---|---|
| **GCC primary** | **Yes** | **Yes** | Good, some lag | High (libgccjit) | Backend instability | **Primary** |
| LLVM primary | **No** (MIPS II only) | Yes | Best | Low | MIPS backend rot | Parallel |
| **Both (ADR-0018)** | **Yes via GCC** | **Yes both** | Best coverage | Highest | Coordination | **Recommended** |

**Recommendation G.1.** **Keep ADR-0018's dual-backend plan: GCC/libgccjit primary, LLVM custom targets parallel.** GCC is the only path that supports o32 + MIPS III and the IRIX ABI faithfully; LLVM gives tooling, `build-std` maturity, and a cross-check. Use LLVM as the CI reference for n32 and GCC as the shipping backend, and run the same test suite through both.

**Open questions and risks.**

- libgccjit build reproducibility and disk footprint; the project reports a disk-space blocker.
- `rustc_codegen_gcc` support for `-Z build-std` on a custom target; test early.
- Debug info and unwinding: the kernel uses `panic = "abort"`, but host-side debugging still needs DWARF from the GCC backend.
- LLVM's o32 limitation forces MIPS II for the ECOFF bootstrap, which is acceptable (the PROM is MIPS II-era anyway) but must be documented.

---

### G.2 Decision: custom target JSON vs official targets

**Decision question.** Should IRIX 7 use official Rust targets (`mips-unknown-linux-gnu`, `mipsel-unknown-none`, `mips-mti-none-elf`) or custom target JSONs?

**Options.**

- **G2-i — Official targets**: `mips-unknown-linux-gnu` (big-endian MIPS, Linux ABI) or `mips-mti-none-elf` (bare-metal MIPS). Pros: maintained, tested, `build-std` works. Cons: wrong ABI (Linux o32, not IRIX n32), wrong OS/environment, no n32, and the Linux target assumes a Linux syscall interface and ELF loader conventions IRIX 7 does not provide.
- **G2-ii — Custom JSON**: `mips3-sgi-irix7-n32` and `mips2-sgi-irix7-o32` (the project's existing files). Pros: exact ABI, endianness, CPU, relocation model, panic strategy; `build-std` supports custom targets. Cons: must be validated field by field; not upstream-tested; some fields (`abi`, `llvm-abiname`) may be ignored or rejected depending on rustc version.
- **G2-iii — Fork rustc to add built-in targets**: `atomchild411/rust-irix` does this for `mips64-sgi-irix` (N32, MIPS IV). Pros: target is first-class, `std` can be built in-tree. Cons: maintaining a rustc fork is a heavy, continuous burden; the project's issue #41 concluded no fork is required.

**Recommendation G.2.** **Custom JSON (G2-ii) with a validation harness**, avoiding a rustc fork. The project has already proven codegen for both targets, and `rust-irixlibstd` demonstrates that JSON targets plus `-Z build-std` are sufficient for real IRIX binaries. Add a CI check that compiles a known program, disassembles it, and asserts the ABI (register usage, struct layout, endianness) rather than trusting the JSON.

**Open questions and risks.**

- The target JSON schema evolves; pin the nightly and re-validate on upgrade.
- `data-layout` strings must match the ABI exactly (`E-m:e-p:32:32-...` for o32; `E-m:e-p:32:32-i64:64-...` for n32); a wrong layout silently miscompiles.
- `max-atomic-width` (32 for o32, 64 for n32) affects `AtomicU64` availability; match the hardware's `ll`/`sc`.
- `features: "+mips3,+soft-float,+noabicalls"` — soft-float is a choice; the Indy has an FPU, so hard-float could be faster but complicates context switching (FPU state) and the ABI. Evaluate.

---

### G.3 Decision: `-Z build-std` vs prebuilt `std`

**Decision question.** Should the kernel and compat layer build `core`/`alloc` (and possibly `std`) from source with `-Z build-std`, or use prebuilt artefacts?

**Options.**

- **G3-i — `-Z build-std=core,alloc,compiler_builtins`**: builds the standard crates for the custom target. Pros: required for a custom target; lets the project patch `core`/`alloc` (e.g. for MIPS atomics); enables `panic_immediate_abort` and size tuning. Cons: needs `rust-src`, nightly, and a host toolchain that can build the target; slower builds; the project reports host-toolchain glue is a blocker (`docs/spec/architecture.md` §2.2).
- **G3-ii — Prebuilt `std` from a rustc fork**: `atomchild411/rust-irix` builds `std` in-tree. Pros: no `build-std` friction; `std` available. Cons: fork maintenance; the kernel does not want `std` anyway.
- **G3-iii — `core` only, vendored**: copy `core` sources. Pros: no nightly dependency; cons: unmaintainable, licence/provenance noise.

**Recommendation G.3.** **`-Z build-std` (G3-i) for the kernel and compat layer**, pinned to a nightly, with `rust-src` vendored for reproducibility. The compat container needs `alloc` but not `std`; a limited `std` port (like `rust-irixlibstd`) is only relevant if IRIX 7 wants to run Rust *user* programs built with `std` — a separate decision.

**Open questions and risks.**

- The nix/rustup self-contained linker glue is a known blocker; a wrapper script or a `rust-toolchain.toml` with the right components must be part of the build.
- `build-std` and `rustc_codegen_gcc` compatibility; test both backends.
- Reproducibility: record the exact nightly, `rust-src` commit, and target JSON in the build metadata.

---

### G.4 Decision: panic strategy

**Decision question.** `panic = "abort"`, unwinding, or a custom panic handler with recovery?

**Options.**

- **G4-i — `panic = "abort"`**: the panic handler prints and halts. Pros: no unwinder, smallest code, deterministic; required on bare metal without an unwinder. Cons: no recovery; a service panic kills the machine (unless caught at a task boundary with a separate mechanism).
- **G4-ii — Unwinding**: Pros: `catch_unwind`, service isolation; cons: needs an unwinder (`unwinding` crate or libgcc), `.eh_frame`/DWARF, and per-task unwinding state; `rust-irixlibstd` had to write a MIPS unwinder because IRIX has no `_Unwind_*` (https://github.com/danifunker/rust-irixlibstd). Heavy for a kernel.
- **G4-iii — Abort at the kernel level, unwind in user tasks**: kernel `panic = "abort"`; user Rust programs may unwind with a user-space unwinder. Pros: kernel simplicity, user ergonomics; cons: two runtimes.

**Recommendation G4.** **`panic = "abort"` for the kernel and ROM** (G4-i), with a per-task fault boundary that kills the offending task and restarts its service cell rather than unwinding. For native user programs, offer unwinding as a library option (G4-iii) using a MIPS unwinder like the one `rust-irixlibstd` wrote, but do not put it in the kernel.

**Open questions and risks.**

- Panic in an interrupt context must not recurse; the handler must be re-entrancy-safe.
- `panic_immediate_abort` for the ROM saves size but removes messages; use a feature flag.
- If a service cell panics, the cell loader/restarter must be able to reclaim its resources without unwinding.

---

### G.5 Decision: o32/MIPS II vs n32/MIPS III for each component

**Decision question.** Which ABI/ISA should each component use: ROM, bootstrap, kernel, compat container, user libraries?

**The constraint.** ARCS is o32/ECOFF and LLVM cannot build o32 + MIPS III, so the ROM/bootstrap is o32/MIPS II. The kernel is n32/MIPS III (ADR-0010). Legacy user binaries may be o32 or n32.

**Recommended allocation.**

| Component | ABI | ISA | Why |
|---|---|---|---|
| PROM-loaded bootstrap | o32 | MIPS II | ARCS/ECOFF; LLVM limitation |
| Kernel | n32 | MIPS III | ADR-0010; 64-bit registers, 32-bit pointers |
| Native services | n32 | MIPS III | Same as kernel |
| Compat container | n32 | MIPS III | Runs in kernel space |
| Legacy o32 binaries | o32 | MIPS II | Their own ABI; kernel decodes both |
| Legacy n32 binaries | n32 | MIPS III | Native n32 |
| ROM (Phase 1) | o32 | MIPS II | PROM-loaded; size/ABI |

**Options.**

- **G5-i — All n32/MIPS III except the bootstrap**: Pros: uniform kernel; cons: o32 user binaries still need o32 decoding, and the bootstrap is the exception.
- **G5-ii — All o32/MIPS II**: Pros: simplest, smallest; cons: 32-bit `off_t`, no 64-bit atomics, contradicts ADR-0010.
- **G5-iii — Mixed as above**: Pros: each component gets the right ABI; cons: two ABIs in one system (already required by compatibility).

**Recommendation G5.** **G5-iii**, exactly as ADR-0010/0016 imply. Keep the ABI boundary explicit in the build system (separate target triples) and never link o32 objects into the n32 kernel.

**Open questions and risks.**

- The o32→n32 transition in the bootstrap: the CPU must be in a known state, and the n32 code must not assume 64-bit pointers (n32 is ILP32).
- `long double` and varargs differ between o32 and n32; the compat container must handle both.
- The kernel's n32 code must still decode o32 syscall frames; the two decoders share the dispatcher (Section C.3).

---

### G.6 Decision: linker and object formats

**Decision question.** Which linker should produce the kernel/ROM images: `rust-lld`, GNU `ld` from binutils 2.47 (IRIX-patched), or a custom linker; and how is ECOFF produced?

**Options.**

- **G6-i — `rust-lld`**: Pros: fast, integrated with rustc, supports custom targets (`linker-flavor: gnu-lld` in the target JSON). Cons: ECOFF output is not supported (LLD emits ELF), so the bootstrap needs a conversion step; MIPS relocation support in LLD is good but IRIX-specific relocations may be missing.
- **G6-ii — GNU `ld` (binutils 2.47 + IRIX patches)**: Pros: native ECOFF and IRIX ELF support; the project already patches binutils (`patches/binutils-2.47/`); `ld` can emit ECOFF for `mips-sgi-irix`. Cons: slower, more build machinery, and the linker becomes part of the toolchain provenance.
- **G6-iii — Hybrid**: LLD for ELF (kernel, user), GNU `ld`/`objcopy` for the ECOFF bootstrap conversion.

**Recommendation G6.** **G6-iii**: use `rust-lld` for the n32 ELF kernel and user binaries, and GNU binutils (with the IRIX patches) to produce/convert the o32 ECOFF bootstrap. Document the conversion command and verify the ECOFF image under the emulator.

**Open questions and risks.**

- ECOFF conversion must preserve the entry point and load address; `objcopy -O ecoff` may lose sections or symbols.
- The PROM may require a specific ECOFF header (a.out magic, `filehdr`/`aouthdr`); verify against the ARCS documentation and a known-good IRIX binary.
- Linker scripts for the kernel must place the exception vectors correctly and align the stack.

---

### G.7 Decision: floating point, atomics, and code model

**Decision question.** Should the kernel use hardware floating point, soft-float, or integer-only; which atomic width; and which code model/relocation model?

**Options.**

- **G7-i — Soft-float**: target JSON already selects `+soft-float`. Pros: no FPU context switching, simpler kernel, deterministic; cons: slow for graphics/media workloads (IRIX identity includes digital media), and compiler-rt/libgcc helpers add code.
- **G7-ii — Hard-float**: Pros: fast FP; cons: must save/restore FPU state on context switch, handle FP exceptions, and match the ABI.
- **G7-iii — Integer-only kernel, hard-float for user**: Pros: kernel simplicity, user performance; cons: two ABIs and a mode switch.

**Atomics.** The R4000/R4400 has `ll`/`sc` (32-bit and 64-bit on MIPS III). `max-atomic-width` should be 64 for n32/MIPS III and 32 for o32/MIPS II. The kernel's `AtomicU64` use must be justified; 64-bit `ll`/`sc` on R4000 may be emulated or slow.

**Code model.** `relocation-model: "static"` and `+noabicalls` in the target JSON avoid GOT/PLT indirection and GP-relative complexity; this suits a kernel linked at a fixed address but complicates position-independent user code and shared libraries.

**Recommendation G7.** **Soft-float for the kernel and ROM initially** (simplest, matches the current target JSON), with hard-float user support and a kernel FPU context-switch path added when graphics/media workloads demand it. Use 64-bit atomics on n32 where the hardware supports them, 32-bit on o32. Keep `static`/`noabicalls` for the kernel; use PIC for user shared libraries.

**Open questions and risks.**

- The Indy's R4000 FPU is external (R4000PC has no FPU; R4000SC/Indy may have R4010). Confirm the actual CPU/FPU configuration.
- `ll`/`sc` atomicity across CPUs on a dual-Indy configuration; the kernel must use the correct cache-op sequences.
- Soft-float `f64` operations call `__divdf3` etc. from compiler_builtins/libgcc; ensure they are linked.

---

### G.8 Decision: prior art — reuse `rust-irixlibstd` / `rust-irix` or build fresh?

**Decision question.** Should IRIX 7 adopt code/ideas from `danifunker/rust-irixlibstd` and `atomchild411/rust-irix`, or build its own targets and libc bindings?

**Options.**

- **G8-i — Adopt `rust-irixlibstd`'s targets and bindings**: it has `mips-sgi-irix` (o32) and `mips64-sgi-irix-n32` (n32) targets, a `libc` module, a MIPS unwinder, and 62 runtime stages verified on IRIX 6.5.22/IP22 (https://github.com/danifunker/rust-irixlibstd). Pros: proven, detailed, layout-asserted; cons: it targets *hosted* IRIX (`std` on IRIX 6.5), not a bare-metal kernel; its ABI assumptions (libc symbols, `/lib/rld`) are for IRIX, not IRIX 7; licence/provenance must be checked.
- **G8-ii — Adopt `atomchild411/rust-irix`**: N32/MIPS IV, rustc fork, std on 1.96.1. Pros: N32 focus, pkgsrc integration; cons: fork maintenance, MIPS IV not III, and the same hosted-vs-bare-metal mismatch.
- **G8-iii — Fresh targets, borrow ABI facts**: write IRIX 7's own target JSONs and layout crate, using the prior art as a cross-check. Pros: clean provenance, exact fit; cons: re-derives known facts.

**Recommendation G8.** **G8-iii**: build fresh, but treat both repositories as **primary references for ABI facts and pitfalls** (o32 vs n32 differences, `off_t`, `sigaltstack` quirks, the missing `*at` family, LLVM's o32 limitation). Their layout assertions and runtime stages are a ready-made test plan. Do not copy code without a licence/provenance review; `rust-irixlibstd`'s licence is not stated in its README (check the repository), and `atomchild411/rust-irix` is a rustc fork with mixed licensing.

**Open questions and risks.**

- Licence compatibility of any borrowed code with GPL-3.0 (ADR-0012).
- The prior art's targets are for hosted IRIX; bare-metal `no_std` targets need different `os`/`env` fields and no libc.
- Their empirical findings (e.g. `SO_RCVTIMEO` rejected, no `*at` family) are valuable compatibility notes for Section C.

---

### G.9 Section G summary

| Decision | Recommended | Why |
|---|---|---|
| Backend | GCC/libgccjit primary, LLVM parallel | GCC supports o32+MIPS III and IRIX ABI; LLVM for tooling/cross-check |
| Target spec | Custom JSON + validation harness, no rustc fork | Proven; avoids fork maintenance |
| Standard library | `-Z build-std` (core, alloc, compiler_builtins) | Required for custom targets; patchable |
| Panic | `panic = "abort"` in kernel/ROM; user unwinding optional | No unwinder in kernel; fault boundary instead |
| ABI allocation | o32/MIPS II bootstrap + ROM; n32/MIPS III kernel | ARCS/LLVM constraint + ADR-0010 |
| Linker | LLD for ELF; GNU binutils for ECOFF conversion | Best of both |
| FP/atomics | Soft-float kernel initially; 64-bit atomics n32 | Simplicity; hardware support |
| Prior art | Fresh targets, borrow ABI facts and test plans | Clean provenance, proven pitfalls |

---

## H. Concurrency and scheduling

### H.1 Decision: injected scheduler trait or built-in scheduler?

**Decision question.** Should scheduling policy live inside the privileged framework, or be an injected `Scheduler`/`RunQueue` implementation outside the TCB?

**Options.**

- **H1-i — Built-in scheduler**: a single policy compiled into the framework. Pros: simplest call path, easiest worst-case analysis, no dynamic dispatch. Cons: every policy change touches the TCB; IRIX's multiple scheduling classes (timeshare, real-time FIFO/RR, batch) become a monolith; the Asterinas result credits policy injection for TCB size.
- **H1-ii — Injected `Scheduler`/`RunQueue` traits**: policy is a safe service; the framework enforces mechanism (context switch, timer, `is_running`). Pros: policies outside the TCB; multiple classes; testable on the host; matches Asterinas §4.4 and the project spec §3.3. Cons: indirect dispatch; a buggy policy can starve tasks; the framework must validate outputs.
- **H1-iii — Boot-time policy selection with static dispatch**: choose a policy type at boot and monomorphise. Pros: no `dyn` cost; multiple policies. Cons: build/config complexity; not all policies can be monomorphised together.

**Recommendation H.1.** **H1-ii with a narrow trait** (`enqueue`, `dequeue`, `pick_next`, `tick`, `set_priority`), implemented by safe services, with boot-time selection and framework validation (every returned task must be runnable and not `is_running`). Consider H1-iii for the hot path if `dyn` overhead is measurable on MIPS.

**Open questions and risks.**

- The trait must be `no_std` and allocation-free in the hot path; run queues are intrusive lists, not `Vec`.
- A malicious/buggy policy can starve the system; the framework should enforce a maximum time slice or a watchdog.
- Real-time policies need priority inheritance or ceiling protocols; those are policy-level but affect framework locks (Section H.4).

---

### H.2 Decision: per-CPU run queues and SMP strategy

**Decision question.** Should run queues be global, per-CPU, or hierarchical; and how should tasks migrate?

**Options.**

- **H2-i — Single global run queue with a lock**: Pros: simple, good load balance; cons: contention, cache-line bouncing, and on a uniprocessor it is just a lock.
- **H2-ii — Per-CPU run queues with work stealing**: Pros: scalable, cache-friendly, standard in Linux/Barrelfish; cons: load imbalance, migration cost, more complex locking.
- **H2-iii — Per-CPU queues with push migration only**: Pros: simpler than stealing; cons: can leave a CPU idle while another has a backlog.
- **H2-iv — Hybrid: per-CPU + global overflow/realtime queue**: Pros: real-time tasks can preempt on any CPU; cons: two paths to reason about.

**Recommendation H.2.** **H2-ii (per-CPU queues with work stealing) for the general scheduler, plus a global real-time queue** for tasks that must run promptly on any CPU. On a single-CPU Indy the per-CPU path degenerates to one queue; the design must not add overhead in that case. Use `is_running` (H.3) to prevent a task from running on two CPUs.

**Open questions and risks.**

- The Indy is usually uniprocessor; SMP is a later concern (dual-CPU Indigo2/Challenge). Do not over-engineer early, but keep the per-CPU structure.
- Work stealing requires atomic queues; MIPS `ll`/`sc` contention.
- CPU affinity and IRIX's `sysmp`/`mpc` facilities may need per-CPU placement controls.

---

### H.3 Decision: the "task runs on at most one CPU" invariant

**Decision question.** How should the kernel enforce Inv. 8 (a task runs on at most one CPU at a time)?

**Options.**

- **H3-i — `is_running` flag checked before context switch**: a per-task atomic flag set on dispatch, cleared on preemption/exit. Pros: cheap, simple, catches double-dispatch. Cons: the window between check and set must be atomic; a stale flag after a crash must be recovered.
- **H3-ii — Per-CPU ownership token**: the task records its owning CPU; only that CPU may run it. Pros: stronger, naturally handles migration; cons: migration requires a handoff protocol.
- **H3-iii — Lock the task during dispatch**: Pros: simple; cons: lock held across context switch is dangerous.

**Recommendation H.3.** **H3-i + H3-ii**: an atomic `is_running` flag plus an `owner_cpu` field; dispatch is a compare-and-swap, and migration is an explicit handoff (clear owner, enqueue on target, target sets owner). This is the Asterinas invariant and is cheap enough for MIPS.

**Open questions and risks.**

- Context switch code must not lose the flag on a fault path; the trap handler must clear it if the task is being descheduled.
- A task that exits while `is_running` must be reaped exactly once.
- Memory ordering: use `Acquire`/`Release` on the flag; MIPS `sync` instructions are needed for published state.

---

### H.4 Decision: real-time scheduling (IRIX identity)

**Decision question.** How should IRIX's real-time scheduling identity be preserved: fixed-priority FIFO/RR classes, a deadline scheduler, or a reservation-based scheme?

**Background.** IRIX offered real-time scheduling classes (`SCHED_FIFO`, `SCHED_RR`) alongside timeshare, with priority bands and `sched_setscheduler`/`sched_getparam`; real-time was a core SGI identity (graphics, media, simulation). The compat layer must reproduce the observable behaviour for legacy binaries; the native scheduler should offer at least as much.

**Options.**

- **H4-i — POSIX fixed-priority FIFO/RR**: strict priority, round-robin within a priority. Pros: matches IRIX and POSIX, simple, predictable; cons: priority inversion unless inheritance/ceiling is added; starvation of lower priorities is by design.
- **H4-ii — Deadline scheduling (EDF)**: Pros: optimal for periodic tasks; cons: not IRIX-compatible, harder to analyse with overruns, and legacy binaries do not use it.
- **H4-iii — Reservation/budgeting (seL4 MCS-style scheduling contexts)**: Pros: temporal isolation, compositional analysis; cons: complex, not IRIX-compatible, and overkill for a workstation.
- **H4-iv — Hybrid: FIFO/RR for legacy + optional deadline class for native**: Pros: compatibility plus modern capability; cons: two schedulers to validate.

**Recommendation H.4.** **H4-i for the compat layer (exact IRIX/POSIX semantics) and H4-iv's optional deadline class for native real-time services.** Implement priority inheritance (or priority ceiling) for kernel locks to bound inversion; document the Mars Pathfinder lesson (https://www.cs.cornell.edu/courses/cs6410/2019fa/slides/24-real-time.pdf; the canonical account is "What Really Happened on Mars?" by Mike Jones). Measure worst-case latency on the emulator and real hardware.

**Open questions and risks.**

- IRIX priority ranges and band semantics must be captured exactly; a wrong mapping breaks legacy real-time programs.
- Priority inversion in the compat container's locks (it shares kernel locks) is a system-wide risk.
- The timer tick vs tickless design affects real-time latency; a tickless kernel with a programmable compare timer is better but more complex.
- FPU/graphics latency: real-time media tasks may need FPU state handling that soft-float avoids.

---

### H.5 Decision: the sproc model

**Decision question.** How should IRIX `sproc` (shared-address-space lightweight processes) be represented natively?

**Background.** `sproc(2)` creates a process that shares the parent's address space (and some resources), the basis of IRIX's `pthread` implementation and its MIPS `mpc` multiprocessing. It is similar to Linux `clone(CLONE_VM|CLONE_FS|CLONE_FILES|...)`.

**Options.**

- **H5-i — Thread group sharing a VMAR**: sproc = a task in the same job/VMAR; `pthread` maps onto it. Pros: matches the native object model (Section B), efficient, natural for SMP. Cons: IRIX's exact sharing set (what is shared vs copied) must be reproduced.
- **H5-ii — Separate process with shared memory segments**: Pros: closer to a classic process model; cons: `sproc` shares the *whole* address space, not selected segments; wrong semantics.
- **H5-iii — Emulate in the container**: Pros: full control; cons: duplicates scheduler/task machinery; poor native integration.

**Recommendation H.5.** **H5-i**: sproc = task group sharing a VMAR (and the file-handle table per IRIX rules), with `pthread` built on the same primitive. Expose it natively as a "share group" so Rust programs can use it without the legacy API.

**Open questions and risks.**

- IRIX sproc sharing flags and `sproc` variants (`PR_SADDR`, `PR_SFDS`, etc.) must be captured; Linux's `clone` flags are a useful analogy but not identical.
- Signal delivery to a share group: which sproc gets it? IRIX's rule must be implemented.
- `sproc` + `fork` interactions (a forked child of a sproc) are subtle.

---

### H.6 Decision: synchronisation primitives

**Decision question.** Which synchronisation primitives should the kernel and native ABI provide?

**Options.**

- **H6-i — Spinlocks + wait queues + futexes**: Pros: minimal, fast uncontended, futexes cover user-space mutexes/condvars; cons: futex is complex (see Drepper, "Futexes Are Tricky", https://www.akkadia.org/drepper/futex.pdf).
- **H6-ii — Semaphores/mutexes/condvars as kernel objects**: Pros: simpler user API, direct IRIX `pthread` mapping; cons: more kernel objects, heavier.
- **H6-iii — Channels/ports only (message passing)**: Pros: fits the capability model, avoids shared-memory races; cons: poor fit for legacy pthreads and for fine-grained locking.

**Recommendation H.6.** **H6-i for the kernel (spinlocks, wait queues, per-CPU deferral) and H6-ii's objects for the native ABI (mutex, condvar, semaphore, event), built on futex-like wait/wake**, with channels/ports for coarse-grained IPC. The compat layer maps IRIX `pthread` primitives onto the native objects.

**Open questions and risks.**

- Priority inheritance for mutexes (H.4).
- Futex on MIPS: atomic wait/wake requires a kernel hash table and `ll`/`sc`; keep it simple.
- Lock ordering and deadlock detection in the framework; document a global lock order.

---

### H.7 Decision: interrupt handling model

**Decision question.** Should interrupts be handled entirely in hard context (top half), split into top/bottom halves, or threaded?

**Options.**

- **H7-i — Top half only**: minimal work in the handler. Pros: simple, low latency; cons: long handlers block interrupts.
- **H7-ii — Top half + deferred work queue**: Pros: short hard handlers, flexible; cons: latency for deferred work, queue management.
- **H7-iii — Threaded interrupts**: handlers run as tasks. Pros: can sleep, use locks; cons: latency and scheduling complexity; needs a real scheduler early.

**Recommendation H.7.** **H7-ii**: hard handlers do the minimum (acknowledge, capture status, enqueue), and deferred work runs in a kernel task. Threaded interrupts (H7-iii) can come later for drivers that need to sleep. On MIPS, keep the exception vector path in assembly + a small Rust dispatcher.

**Open questions and risks.**

- The Indy's HPC3 interrupt controller and the CPU's interrupt mask/status registers must be programmed correctly; spurious interrupts are common in emulators.
- Interrupt latency vs throughput trade-off; measure with a timer.
- Deferred work must not be starved by a runaway real-time task.

---

### H.8 Section H summary

| Decision | Recommended | Why |
|---|---|---|
| Scheduler policy | Injected traits, boot-time selection, framework validation | Small TCB; multiple IRIX classes |
| Run queues | Per-CPU + work stealing; global RT queue | Scalable, cache-friendly, RT preemption |
| One-CPU invariant | `is_running` atomic + `owner_cpu` | Cheap, matches Asterinas Inv. 8 |
| Real-time | FIFO/RR for compat; optional deadline class | IRIX identity + modern capability |
| sproc | Thread group sharing a VMAR | Native object reuse; pthread basis |
| Synchronisation | Spinlocks/wait queues/futex + native mutex/condvar objects | Minimal kernel, legacy mapping |
| Interrupts | Top half + deferred work queue | Low latency, flexible |

---

## I. Device drivers

### I.1 Decision: how should drivers access MMIO and PIO?

**Decision question.** Should drivers use raw pointers/`volatile`, safe `IoMem`/`IoPort` wrappers, or a memory-mapped register abstraction generated from a description?

**Options.**

- **I1-i — Raw pointers with `volatile` reads/writes**: Pros: direct, zero abstraction, matches C drivers. Cons: requires `unsafe` in every driver, breaks the framekernel split; `volatile` does not prevent aliasing UB or out-of-bounds access; a wrong offset silently corrupts unrelated registers.
- **I1-ii — `IoMem`/`IoPort` safe wrappers**: a region object with bounds-checked `read32`/`write32` (and narrower widths), volatile semantics, and no Rust references into device memory. This is Asterinas's model (arXiv:2506.03876). Pros: drivers stay in safe Rust; offsets checked; no aliasing UB; the framework owns the sensitive I/O labels. Cons: per-access bounds checks; the API must cover read-modify-write and barriers; not all devices fit a flat register window.
- **I1-iii — Register descriptions (generated from SVD/DT)**: Pros: type-safe register fields, documentation; cons: tooling and descriptions for 1990s SGI devices do not exist; overkill.
- **I1-iv — `UnsafeCell` + macros**: Pros: low ceremony; cons: still unsafe, no bounds checks.

**Recommendation I1.** **I1-ii (`IoMem`/`IoPort`) as the only way drivers touch registers**, with typed accessors (`read32`, `write32`, `update32`), explicit barriers, and endianness-aware accessors (the Indy's devices are big-endian, but some may not be). Drivers may not hold references into `IoMem`; they copy values. This is the single biggest lever for keeping drivers out of the TCB.

**Open questions and risks.**

- Some devices require multi-register sequences that must not be interrupted; provide a `with_lock`/critical-section API rather than raw access.
- MMIO on MIPS may need uncached (`kseg1`) mappings; the `IoMem` constructor must enforce that.
- Register width/alignment: 8/16/32-bit accesses to the wrong width can fault or misbehave on some buses.

---

### I.2 Decision: DMA mapping model

**Decision question.** How should drivers do DMA safely: raw physical addresses, `DmaCoherent`/`DmaStream` typed mappings, or a scatter-gather list abstraction?

**Options.**

- **I2-i — Raw physical addresses**: Pros: simple, matches datasheets. Cons: driver can program any address; no lifetime tracking; cache maintenance is manual and easy to forget; a bug becomes memory corruption.
- **I2-ii — `DmaCoherent`/`DmaStream` over untyped memory** (Asterinas): `DmaCoherent` for long-lived coherent buffers, `DmaStream` for streaming mappings with explicit ownership transitions and cache maintenance. Pros: the type system ties a DMA address to a live buffer; only untyped memory can be DMA-mapped (Inv. 6); cache ops are part of the API. Cons: more types; streaming ownership rules must be learned; not all devices fit.
- **I2-iii — Scatter-gather lists**: Pros: handles fragmented buffers and large transfers; cons: the Indy's devices may not support SG; descriptor management is complex.
- **I2-iv — Bounce buffers only**: Pros: device can only reach a reserved region; cons: copy cost, and still needs typed addresses to avoid misuse.

**Recommendation I2.** **I2-ii (`DmaCoherent`/`DmaStream`) over untyped memory, with a bounce-buffer fallback for devices with address limitations.** Pair every mapping with architecture-specific cache maintenance (`wback`/`invalidate` on R4000-class CPUs), and make the DMA address unforgeable (a newtype that only the framework can construct). This is the safe-driver design that Tock/RedLeaf/Theseus lack, per the Asterinas paper's argument.

**Open questions and risks.**

- Cache coherency: R4000 caches are not coherent with DMA; the API must require `clean` before device read and `invalidate` after device write, and must not allow CPU access during a streaming mapping.
- The Seeq 80C03 and HPC3 DMA engines may have alignment/length constraints; encode them in the typed API.
- IOMMU absence means a buggy *device* can still write anywhere; typed mappings protect against buggy *drivers*, not malicious hardware (Section I.4).

---

### I.3 Decision: safe drivers vs unsafe-heavy drivers

**Decision question.** Can real drivers for 1990s SGI hardware be written in safe Rust, or is `unsafe` unavoidable?

**The evidence.** Tock, RedLeaf, and Theseus all report substantial `unsafe` in drivers because their kernel APIs cannot express MMIO/DMA safely (Asterinas's motivation). Asterinas claims drivers can be safe Rust on top of `IoMem`/`DmaCoherent`/`DmaStream`, with the framework owning the sensitive operations.

**Options.**

- **I3-i — All drivers safe Rust on framework APIs**: Pros: drivers outside the TCB; the framekernel claim holds. Cons: the framework API must cover every device quirk; some drivers may need a small `unsafe` shim.
- **I3-ii — Drivers may use `unsafe`, but in a separate crate with review**: Pros: pragmatic; cons: TCB grows; the boundary becomes fuzzy.
- **I3-iii — Drivers in userspace**: Pros: hardware isolation; cons: rejected (Section A) for IPC cost.

**Recommendation I3.** **I3-i with an explicit escape hatch**: every driver must be safe Rust; if a device genuinely needs `unsafe`, the unsafe code goes into a *framework* driver-support module (reviewed, counted in the TCB), not into the driver crate. Publish the per-driver `unsafe` count (ideally zero).

**Open questions and risks.**

- The Indy's devices are quirky (HPC3 DMA, Seeq, WD33C93 SCSI, Newport graphics); the framework API may need device-specific hooks.
- Interrupt handlers often need `unsafe` for register access; route them through `IoMem`.
- Some drivers need timing loops (`udelay`); provide a safe busy-wait API.

---

### I.4 Decision: IOMMU substitute on the Indy

**Decision question.** The Indy has no IOMMU. What protects memory from buggy or malicious DMA?

**The reality.** On MIPS Indy, bus-mastering devices (Seeq Ethernet, SCSI, graphics) program physical addresses directly. Without an IOMMU, a device can write any physical address the driver programs into its DMA descriptors. The threat model splits into (a) buggy drivers, which typed DMA mappings mitigate, and (b) malicious devices, which no software can fully mitigate without an IOMMU.

**Options.**

- **I4-i — Typed DMA mappings only**: the driver can only obtain DMA addresses for buffers it owns; the framework validates. Pros: zero cost, catches driver bugs; cons: does not stop a malicious device or a wild descriptor fetch.
- **I4-ii — Bounce buffers / reserved DMA region**: all DMA goes through a reserved physical region; the framework copies to/from kernel buffers. Pros: confines device writes to the region (if every descriptor is validated); cons: copy cost, and a malicious device can still be programmed with an out-of-region address by a compromised driver.
- **I4-iii — Software IOMMU (shadow page tables)**: maintain a translation table and validate every DMA address at map time, but the hardware still uses physical addresses; only works if the device's descriptors are rewritten (bounce) or if the bus has an address-translation window. Pros: closest to an IOMMU; cons: effectively bounce buffering with extra bookkeeping.
- **I4-iv — Restrict DMA-capable memory at boot**: only a small low-memory pool is ever given to devices; the kernel and user memory are outside the device's reachable range? On a 32-bit physical address space, devices can reach all RAM; no.
- **I4-v — Disable bus mastering for untrusted devices**: Pros: absolute; cons: device unusable.

**Recommendation I4.** **I4-i + I4-ii**: typed DMA mappings as the primary defence, with a reserved bounce region for devices that cannot address the full range or that are considered risky. Document that a malicious device is out of scope (physical access is required to install one). Add a boot-time check that all DMA-capable devices' descriptor registers are only programmed through the framework.

**Open questions and risks.**

- The HPC3 may have address-translation registers for some channels; investigate the hardware documentation.
- Cache maintenance and bounce buffers interact (copy must be cache-coherent).
- A future IOMMU-like device (or emulator feature) could strengthen this; keep the DMA API abstract enough to add translation later.

---

### I.5 Decision: driver model and the Indy device set

**Decision question.** Should drivers be in-kernel cells, loadable modules, or a hybrid; and which devices are in the first milestone?

**Options.**

- **I5-i — In-kernel statically linked drivers**: Pros: simplest, no loader in the TCB; cons: no runtime replacement.
- **I5-ii — Loadable driver cells**: Pros: live update, fault recovery; cons: loader complexity (Section A.4).
- **I5-iii — Hybrid: core drivers built-in, optional drivers loadable**: Pros: pragmatic; cons: two paths.

**Recommendation I5.** **I5-iii**: build the boot-critical drivers (serial, timer, interrupt controller, HPC3, Seeq, SCSI, RTC) into the kernel; make non-critical drivers loadable cells later. Prioritise the devices needed for the tracer bullet and the compat layer: serial console, timer, HPC3 interrupt controller, Seeq Ethernet (for the ROM/kernel networking), SCSI disk (for the root filesystem), RTC (time), and Newport graphics (console/graphics) last.

**Device notes (Indy/IP22).**

- **HPC3**: the system I/O controller; interrupt controller, DMA, Ethernet, SCSI, serial, audio, and parallel all hang off it. Its register block is memory-mapped (the prompt gives the Seeq base as `0xBF580000`); the framework must label these ranges sensitive.
- **Seeq 80C03 Ethernet**: 10 Mbit/s, DMA via HPC3; the ROM and kernel share a driver.
- **WD33C93 SCSI**: the Indy's SCSI controller; DMA, disconnect/reselect, and a quirky register interface.
- **z85c30 SCC**: serial; polled early console, interrupt-driven later.
- **Dallas DS1386 RTC/NVRAM**: time and NVRAM; battery may be dead (Section F.4).
- **Newport/GR2 graphics**: the Indy's graphics; complex, high-risk, and best deferred until the system can boot and run console programs.
- **HAL2 audio**: deferred.

**Reference implementations (behavioural, not copied).** Linux `arch/mips/sgi-ip22` and `drivers/net/ethernet/seeq/seeq8005.c`; NetBSD `sys/arch/sgimips/`; OpenBSD `sys/arch/sgi/`; the `hinv` output on a real machine or the emulator. These are references for register behaviour and sequencing under the clean-room rules; no code is copied.

**Open questions and risks.**

- The emulator (`irix7/iris`, a fork of `techomancer/iris`) may not model every device identically; cross-check on real hardware where possible.
- HPC3 DMA descriptors and cache coherency are the most likely source of early bugs.
- SCSI disk geometry and the root filesystem layout must be decided before the compat layer can run real binaries.

---

### I.6 Section I summary

| Decision | Recommended | Why |
|---|---|---|
| MMIO/PIO | `IoMem`/`IoPort` only | Keeps drivers safe; bounds and endianness checked |
| DMA | `DmaCoherent`/`DmaStream` over untyped memory | Lifetime-safe, cache ops built in |
| Driver trust | Safe Rust; `unsafe` only in framework support | Preserves the framekernel split |
| IOMMU substitute | Typed DMA + bounce region; malicious devices out of scope | Best available without hardware translation |
| Driver model | Core built-in, optional loadable cells later | Boot simplicity + future flexibility |
| Device order | Serial, timer, HPC3, Seeq, SCSI, RTC, graphics last | Tracer bullet and compat layer first |

---

## J. Filesystems and identity services

### J.1 Decision: XFS — clean-room reimplementation, port, or substitute?

**Decision question.** How should IRIX 7 provide XFS, given the clean-room and GPL-3.0 constraints: reimplement XFS clean-room, port the existing open-source XFS, use a different filesystem, or support XFS only for legacy volumes?

**Background.** XFS originated at SGI for IRIX, was open-sourced in 2000, and is now a GPL-licensed Linux filesystem (https://xfs.org/; https://docs.kernel.org/filesystems/xfs-maintainer-entry-profile.html). Its design is documented in "Scalability in the XFS File System" (Sweeney et al., USENIX ATC 1996, https://www.usenix.org/legacy/publications/library/proceedings/sd96/full_papers/sweeney.pdf): allocation groups for parallelism, B+ trees for inodes and extents, delayed allocation, journaling (the log), and 64-bit metadata. XFS is IRIX identity: IRIX 5.3 introduced it, and IRIX 6.5's root and data volumes are XFS.

#### Option J1 — Clean-room reimplementation of XFS

Implement the on-disk format and semantics from published descriptions, the oracle, and behavioural testing, with no SGI/Linux XFS code copied.

**Pros.**

- Satisfies ADR-0009/0014 (clean-room) and ADR-0012 (GPL-3.0) without licence entanglement.
- Full control over the implementation: Rust, safe where possible, integrated with the native VFS and capability model.
- Can be scoped: read/write of the common case, with journal replay and recovery implemented incrementally.
- The on-disk format is well documented (SGI papers, Linux documentation, `xfs_db` behaviour) and can be validated against real IRIX volumes.

**Cons.**

- XFS is large: allocation groups, B+ trees (inode, free space, reverse mapping), extents, delayed allocation, journaling, CRCs (v5), quotas, ACLs, extended attributes, real-time subvolume, and more.
- Clean-room reimplementation of a journaled FS is a multi-year effort; crash consistency is hard to test.
- Subtle behaviours (log replay ordering, inode fork formats, btree key ordering) are easy to get wrong and catastrophic when wrong.
- Performance parity with SGI's XFS is unlikely without years of tuning.

#### Option J1b — Port the existing open-source XFS

Port Linux's XFS (GPL-2.0) to IRIX 7's kernel interfaces.

**Pros.**

- Mature, tested, performant; huge test suite (`xfstests`).
- Licence-compatible with GPL-3.0 (GPL-2.0-only is incompatible with GPL-3.0 in the strict sense; GPL-2.0-or-later is compatible — Linux's XFS is GPL-2.0-only, so this is a real problem). Even if relicensed, porting C code into a Rust kernel requires an `unsafe` C FFI layer, inflating the TCB and breaking the framekernel model.
- Contradicts the clean-room mandate (it is SGI-derived expression, even if open-sourced).

**Cons.**

- Licence: GPL-2.0-only is not compatible with GPL-3.0 for combined distribution; the project would need permission or a relicence.
- Clean-room violation: the code is SGI-derived.
- C in a Rust kernel: `unsafe` everywhere, no type safety, and a large new TCB.

#### Option J1c — Use another filesystem natively, support XFS read-only for legacy volumes

Native FS choice: ext4 (GPL-2.0, complex), btrfs (GPL-2.0, complex), F2FS (GPL-2.0), ZFS (CDDL — incompatible with GPL-3.0), or a new simple FS.

**Pros.**

- Much less work than XFS; a simple extent-based FS can be written in safe Rust in a fraction of the time.
- Legacy XFS volumes can be mounted read-only via a compatibility driver, or accessed through the compat container's original XFS code? (No — no SGI code.)
- Clear separation: native FS for IRIX 7's own volumes, XFS compatibility for IRIX 6.5 data.

**Cons.**

- Abandons XFS as a first-class identity service (contradicts the project brief).
- Two filesystems to maintain.
- ZFS's CDDL is a licence trap; avoid.
- ext4's journal and feature flags are as complex as XFS in different ways.

#### Option J1d — XFS-compatible clean-room implementation, scoped

A clean-room XFS implementation that starts read-only (mount, read, lookup, extents) and grows write support, journal replay, and CRCs incrementally.

**Pros.**

- Satisfies identity and clean-room; delivers value early (read-only mount of IRIX volumes).
- Testable against real images and `xfstests` (adapted).
- Write support can be added once the read path is proven.

**Cons.**

- Still a large long-term effort; read-only is not enough for a root filesystem.
- Journal replay is needed even for read-only mounts of unclean volumes.

#### J.1 comparison table

| Option | Clean-room | Licence | Effort | XFS identity | TCB impact | Fit |
|---|---|---|---|---|---|---|
| **Clean-room XFS (scoped, read-first)** | **Yes** | **GPL-3.0 OK** | **Very high** | **Preserved** | **Rust, low** | **Strategic** |
| Port Linux XFS | No | GPL-2.0-only conflict | Medium | Preserved | High (C FFI) | Rejected |
| Other FS + XFS RO | Yes (for new FS) | Depends | Medium | Partial | Low | Pragmatic fallback |
| XFS-compatible RO first | Yes | GPL-3.0 OK | High | Preserved | Low | **First milestone** |

**Recommendation J.1.** **Clean-room XFS, staged: read-only mount and file access first, then journal replay for unclean volumes, then write support.** In parallel, define a simple native filesystem (or use the read-only extent layout from Section F.10) for IRIX 7's own boot/root volume so the system is usable before XFS write support lands. Do not port Linux XFS (licence and clean-room conflicts); do not adopt ZFS (CDDL).

**Open questions and risks.**

- XFS v5 CRCs and metadata checksums must be verified on read; a wrong checksum implementation rejects valid volumes.
- Allocation group and B+ tree traversal must be robust against corrupt/malicious images (fuzzing).
- Journal replay semantics: the log is a circular buffer with its own record format; replay order matters.
- Real-time subvolumes and DMAPI (data management) are advanced features; scope them out initially.
- Performance: XFS's allocation-group parallelism matters on multi-disk systems; the Indy is single-disk, so a simpler allocator may suffice while keeping the on-disk format compatible.

---

### J.2 Decision: VFS and file-object model

**Decision question.** Should the native filesystem interface be path-based (UNIX `open("/a/b")`) or capability-based (handles to file objects, with a namespace service resolving paths)?

**Options.**

- **J2-i — Path-based VFS**: Pros: familiar, direct compat mapping; cons: ambient authority, TOCTOU races on path resolution, symlink attacks.
- **J2-ii — Capability-based**: `open` returns a handle; directories are handles that can be traversed with `openat`-style operations; the compat layer gets a namespace service that resolves paths to capabilities using the process's root/cwd handles. Pros: no ambient authority, race-free traversal, fits the native model; cons: more complex, legacy `open` needs a resolver, and absolute paths require a root capability.
- **J2-iii — Hybrid**: path-based for legacy, capability-based for native, sharing the same underlying filesystem objects. Pros: pragmatic; cons: two APIs and a risk of privilege leakage at the boundary.

**Recommendation J2.** **J2-ii (capability-based) for the native ABI, with J2-iii's compat resolver**: the native API exposes directory/file handles with rights; the compat container implements `open`/`stat`/`readdir` by resolving paths against the process's root and cwd capabilities. This is the Genode/Fuchsia approach (Genode's VFS and ROM/file-system sessions; Fuchsia's namespace) and avoids ambient authority.

**Open questions and risks.**

- Symbolic links and `..` must be resolved without escaping the root capability; use `openat2`-style `RESOLVE_BENEATH` semantics.
- The compat layer's `chroot`, `chdir`, and `fchdir` need careful handle management.
- Mount points: who owns the namespace, and how are new filesystems attached? A namespace service with explicit mount capabilities.

---

### J.3 Decision: identity services as first-class — which ones and how?

**Decision question.** Which IRIX identity services should be first-class in IRIX 7, and what modern designs should they follow?

**The identity list (from the brief):** `sproc`, real-time scheduling, graphics and digital media, hardware inventory (`hinv`), topology awareness. STREAMS is legacy-compat only.

| Service | IRIX role | Modern reference(s) | IRIX 7 design |
|---|---|---|---|
| `sproc` | Shared-address-space processes; basis of pthreads and MIPS multiprocessing | Linux `clone`, POSIX threads, Fuchsia threads | Native share group over a VMAR (Section H.5) |
| Real-time scheduling | FIFO/RR classes, priority bands, deterministic latency | POSIX `sched_*`, PREEMPT_RT, seL4 MCS | Injected scheduler classes (Section H.4) |
| Graphics/media | GL, OpenGL, VideoLibrary, audio, media pipelines | Vulkan, Mesa, GStreamer, PipeWire | Native media/graphics services; legacy GL compat later |
| Hardware inventory (`hinv`) | Enumerates CPUs, memory, disks, devices | ACPI, devicetree, sysfs, SMBIOS, `hwloc` | A hardware graph object built at boot, exposed natively and via a `hinv`-compatible tool |
| Topology awareness | CPU/memory/device locality, placement | `hwloc`, ACPI SRAT, devicetree NUMA | Topology as a first-class object; scheduler and allocator consume it |
| STREAMS | Kernel stream modules, networking | io_uring, netlink, sockets, DPDK | Legacy-compat shim only; native I/O uses channels/ports + async objects |

**Options for each.**

- **J3-i — Reimplement as native objects with modern semantics**: Pros: coherent, capability-friendly; cons: legacy tools need shims.
- **J3-ii — Emulate the legacy interface only**: Pros: compatibility; cons: no modern capability, and the "first-class" claim is false.
- **J3-iii — Hybrid: native object + legacy view**: Pros: both; cons: two surfaces.

**Recommendation J3.** **J3-iii for all five**: each identity service is a native object/service with a modern design, plus a legacy-compatible view for IRIX binaries. Concretely:

- **Hardware inventory**: build a device graph at boot from the PROM's `hinv` data and bus probing; expose it as a read-only capability object; provide a `hinv`-compatible command in the compat layer. References: devicetree (https://www.devicetree.org/), ACPI, `hwloc` (https://www.open-mpi.org/projects/hwloc/).
- **Topology**: model CPU/memory/device locality; feed the scheduler's per-CPU queues and the frame allocator's locality preferences. Reference: `hwloc`.
- **Real-time**: scheduler classes as above.
- **sproc**: share groups as above.
- **Graphics/media**: start with a framebuffer console and a 2D service; add GL compatibility via a clean-room GL implementation or a port of Mesa (MIT licence, GPL-compatible) later. Media: a pipeline service inspired by GStreamer/PipeWire, with IRIX media libraries as compat surfaces.
- **STREAMS**: a compatibility shim for `open`/`ioctl`/`putmsg`/`getmsg` on legacy devices; do not build new native interfaces on it.

**Open questions and risks.**

- The `hinv` data format and the PROM's device tree must be captured exactly; the emulator may differ from real hardware.
- Graphics is the largest identity service and the least essential for early milestones; defer.
- Topology on a single-CPU Indy is trivial; design for the dual-CPU case without over-engineering.
- STREAMS compatibility may be deeper than expected (networking modules, `ldterm`, `ttcompat`); scope it to what legacy binaries actually use.

---

### J.4 Decision: modern equivalents catalogue (ADR-0017)

**Decision question.** Which modern open-source projects should serve as references for each IRIX component, under the clean-room rules?

**Catalogue (reference only; no code copied without licence/provenance review).**

| IRIX component | Modern reference(s) | What to learn | Licence note |
|---|---|---|---|
| XFS | Linux XFS docs, `xfs_db`, XFS papers | On-disk format, allocation groups, B+ trees, log | Linux XFS GPL-2.0-only (reference only) |
| `sproc`/pthreads | Linux `clone`, POSIX pthreads, Fuchsia threads | Sharing flags, TLS, signal delivery | Specifications are open |
| Real-time scheduling | POSIX `sched_*`, PREEMPT_RT, seL4 MCS | Priority classes, inversion avoidance, budgets | GPL-2.0 (reference) |
| STREAMS | Solaris STREAMS docs, Linux `io_uring`, netlink | Message-based I/O, backpressure | Mixed |
| Graphics | Mesa, Vulkan, OpenGL specs | Driver model, command submission | MIT/Apache (Mesa) |
| Media | GStreamer, PipeWire, FFmpeg | Pipeline graphs, buffer sharing | LGPL/GPL (check) |
| Hardware inventory | devicetree, ACPI, sysfs, SMBIOS, `hwloc` | Device graph, properties, topology | BSD/GPL mix |
| Topology | `hwloc`, ACPI SRAT, devicetree NUMA | Locality, CPU/memory maps | BSD |
| Networking | smoltcp, lwIP, Linux net | Stack structure, buffer management | 0BSD/BSD/GPL |
| Capability model | seL4, Fuchsia/Zircon, Genode | Objects, rights, handles, VMAR/VMO | BSD/MIT/GPL mix |
| Framekernel | Asterinas/Ostd | TCB split, safe APIs, invariants | MPL-2.0? (check) |
| Cells/SAS | Theseus | Cell loading, MappedPages, namespaces | MIT |
| Boot | phil-opp, Theseus, Limine | `no_std` entry, linker scripts, boot protocols | MIT/Apache/BSD |
| TLS | rustls, webpki, embedded-tls | `no_std` TLS 1.3, certificate validation | Apache-2.0/ISC/MIT |
| Compression | miniz, zlib-ng, lz4 | Read-only FS decompression | MIT/BSD/zlib |
| Filesystem images | SquashFS, EROFS, ISO 9660 | Read-only layouts, extent maps | GPL-2.0 (reference) |

**Recommendation J.4.** Maintain this catalogue as a living ADR-0017 document with, for each entry, the licence, the exact things to learn, and the clean-room boundary (specification vs code). Prefer specifications and papers over source code; when source is consulted, record it in the dirty/clean split (ADR-0014) and ensure only behavioural descriptions reach the clean side.

**Open questions and risks.**

- Licence compatibility is per-component, not per-project; a project may have mixed licences.
- Some "references" (Linux XFS, PREEMPT_RT) are GPL-2.0-only; reading them is fine, copying is not, and the clean-room process must be auditable.
- The catalogue must not become a shopping list for porting; the default is reimplementation from specification.

---

### J.5 Decision: root filesystem and crash consistency

**Decision question.** What should the root filesystem be, and how should crash consistency be handled?

**Options.**

- **J5-i — XFS root**: Pros: identity, one FS; cons: XFS write support is the hardest part and arrives last.
- **J5-ii — Simple native FS root + XFS data volumes**: Pros: usable early, XFS for legacy data; cons: two FS.
- **J5-iii — Read-only image root + RAM overlay**: Pros: immutable, verifiable, simple; cons: writes lost on reboot unless persisted.
- **J5-iv — FAT32/ext4 root**: Pros: tooling; cons: not identity, licence/complexity.

**Recommendation J5.** **J5-iii for early bring-up (read-only image + RAM overlay), migrating to J5-ii (native FS root, XFS data volumes) as XFS matures.** Crash consistency: use a journal for the native FS, and for XFS implement log replay before enabling write support. Test with fault injection (power-cut simulation) and `xfstests` where applicable.

**Open questions and risks.**

- The overlay must be able to persist to disk once a writable FS exists.
- Journal replay on mount must be bounded and resumable; a corrupt log must not hang the boot.
- fsck equivalents: offline repair tools are needed for a shipping system.

---

### J.6 Section J summary

| Decision | Recommended | Why |
|---|---|---|
| XFS | Clean-room, read-only first, then replay/write | Identity + clean-room + licence |
| VFS | Capability-based native; path resolver in compat | No ambient authority; race-free |
| Identity services | Native object + legacy view for all five | Modern design with compatibility |
| ADR-0017 catalogue | Specifications/papers first; source as reference only | Clean-room auditability |
| Root FS | Read-only image + overlay early; native FS + XFS data later | Usable early, identity long-term |

---

## K. Summary of recommendations

### K.1 Decision matrix

| # | Decision | Recommended option | Primary justification | Key risk |
|---|---|---|---|---|
| A.1 | Kernel shape | Framekernel + cell-structured monolith | ~14% published TCB, monolithic performance, IRIX semantics | Framework/services boundary erosion |
| A.2 | Isolation | Hybrid: SAS/SPL for Rust, MMU for legacy processes | Trust the compiler for verified code, not 1990s binaries | Two worlds, two entry paths |
| A.3 | Crate depth | Deep crates, `forbid(unsafe_code)` on services | Mechanical TCB boundary; cells | Cross-crate codegen on MIPS |
| A.4 | Cells | Cells ≈ crates; static early, runtime loader later | Benefits without early loader risk | MIPS relocation handling |
| B.1 | Native ABI | Hybrid capability (seL4 semantics, Zircon ergonomics) | Satisfies ADR-0011; one authority model | Interface-ID versioning |
| B.2 | Object set | Five core + events/futex/timer, grow later | Small enough to build, rich enough for runtimes | Rights-mask design |
| B.3 | Stable ABI | `repr(C)` at trap boundary; `abi_stable` for rich cell interfaces | Stability where needed, low hot-path cost | `abi_stable` `no_std` on MIPS |
| B.4 | Native invocation | Capability `invoke` only; typed wrappers | No ambient authority | Batching for I/O |
| C.1 | Compat form | In-kernel safe-Rust container | Cheapest syscall path; full semantics | Container bug = kernel bug |
| C.2 | Layer order | Layout crate → syscall ABI → loader → libc | Testability | Generated layout provenance |
| C.3 | o32/n32 | Two decoders → one dispatcher | Exact conventions; shared code | IRIX error convention |
| C.4 | Object formats | ECOFF boot, ELF user | PROM + IRIX 6.5 reality | o32→n32 transition |
| C.5 | libc | Clean-room Rust + generated layouts + oracle tests | Licence-clean, high fidelity | `FILE`/locale/varargs quirks |
| C.6 | Process model | Native jobs + VMAR cloning; sproc = shared VMAR | Reuses native objects | IRIX sharing semantics |
| D.1 | Memory core | Refcounted `Frame`/`Segment` + `MappedPages` | Sound references + correct sharing | Metadata overhead |
| D.2 | Metadata/invariants | Per-frame metadata; types + runtime checks; Inv. 1–10 | Enforceable, auditable | Panic-on-violation policy |
| D.3 | External memory | `IoMem`/`IoPort` + `UFrame`/`USegment` POD | Safe drivers, sound vs devices | Cache coherency |
| D.4 | Sensitivity | Per-frame flag, framework-only transitions | Basis of Inv. 4–7 | Complete classification |
| D.5 | Native memory | VMAR/VMO | Clean sharing/delegation | `MAP_FIXED` semantics |
| D.6 | TLB/cache | Hybrid direct map + TLB; explicit cache ops | Performance/correctness on R4000 | Cache aliasing |
| E.1 | Trust split | Framework holds `unsafe`; services safe | Smallest TCB | API expressiveness |
| E.2 | Policy | Injected scheduler/allocator traits | Policy out of TCB | Starvation by policy |
| E.3 | Language isolation | Framework checks + MMU for legacy | Sound where types suffice | Rust soundness bugs |
| E.4 | Invariants | Types + runtime checks | Practical and auditable | Release-check coverage |
| E.5 | Stacks | Guard pages + static analysis + canaries | Robust on small RAM | GCC stack-size metadata |
| E.6 | Assurance | Miri, Kani, fuzzing | Best value | Asm/MMIO limits |
| F.1 | Boot path | PROM + o32 ECOFF baseline; two-phase ROM optional | Proven path; ROM as experiment | TLS/size for ROM |
| F.2 | ROM structure | Feature-gated single image | Testable, size-tunable | 512 KiB budget |
| F.3 | ROM stack | smoltcp | Bare-metal fit, 0BSD | Buffer sizing |
| F.4 | TLS | Signed kernel + HTTP/TFTP for ROM; full TLS later | Removes dead-RTC chicken-and-egg | Pin/CA rotation |
| F.5 | Endianness | Explicit `to_be`/`from_le_bytes` at boundaries | Correct on big-endian MIPS | Missed fields |
| F.6 | Boot sequence | Asm → vectors/TLB → `_start` → `nano_core` → `captain` | Small auditable asm | PROM entry state |
| F.7 | no_std | `no_std` + `alloc` kernel; no-alloc ROM; `panic=abort` | Bare-metal reality | `compiler_builtins` |
| F.8 | Console | PROM first, then polled SCC | Earliest marker | SCC registers |
| F.9 | LFS block device | Block cache + read-ahead + Range + refresh | Avoids whole-file download | Signed-URL expiry |
| F.10 | Disk format | Custom read-only extents; ISO 9660 fallback | Tiny parser, one request/extent | Tooling |
| G.1 | Backend | GCC primary, LLVM parallel | o32+MIPS III; IRIX ABI; cross-check | libgccjit build |
| G.2 | Target spec | Custom JSON + validation, no fork | Proven; low maintenance | Schema drift |
| G.3 | Std | `-Z build-std` | Required for custom targets | Host linker glue |
| G.4 | Panic | `panic=abort`; optional user unwinding | No unwinder in kernel | Fault-boundary design |
| G.5 | ABI allocation | o32/MIPS II boot+ROM; n32/MIPS III kernel | ARCS/LLVM + ADR-0010 | o32→n32 handoff |
| G.6 | Linker | LLD for ELF; GNU binutils for ECOFF | Best of both | ECOFF conversion |
| G.7 | FP/atomics | Soft-float kernel; 64-bit atomics n32 | Simplicity | FPU configuration |
| G.8 | Prior art | Fresh targets, borrow ABI facts/tests | Clean provenance | Licence review |
| H.1 | Scheduler policy | Injected traits, boot-time selection | Small TCB; IRIX classes | Policy starvation |
| H.2 | Run queues | Per-CPU + stealing; global RT queue | Scalable, RT preemption | UP overhead |
| H.3 | One-CPU invariant | `is_running` atomic + `owner_cpu` | Cheap, matches Inv. 8 | Fault-path clearing |
| H.4 | Real-time | FIFO/RR compat; optional deadline class | IRIX identity + modern | Priority inversion |
| H.5 | sproc | Thread group sharing a VMAR | Native reuse | IRIX sharing flags |
| H.6 | Synchronisation | Spinlocks/futex + native mutex/condvar | Minimal kernel | Futex complexity |
| H.7 | Interrupts | Top half + deferred work | Low latency | Controller quirks |
| I.1 | MMIO/PIO | `IoMem`/`IoPort` only | Safe drivers | Multi-register sequences |
| I.2 | DMA | `DmaCoherent`/`DmaStream` over untyped | Lifetime-safe, cache ops | Non-coherent caches |
| I.3 | Driver trust | Safe Rust; `unsafe` only in framework | Preserves split | Device quirks |
| I.4 | IOMMU substitute | Typed DMA + bounce region | Best without hardware | Malicious devices |
| I.5 | Driver model | Core built-in, optional loadable cells | Boot simplicity + flexibility | HPC3 DMA |
| J.1 | XFS | Clean-room, read-only first | Identity + clean-room + licence | Log replay |
| J.2 | VFS | Capability-based native; path resolver in compat | No ambient authority | Root/`..` escapes |
| J.3 | Identity services | Native object + legacy view | Modern + compatible | Graphics scope |
| J.4 | ADR-0017 | Specs/papers first; source reference only | Clean-room auditability | Licence mix |
| J.5 | Root FS | RO image + overlay early; native FS + XFS later | Usable early | Persistence |

### K.2 Why this combination fits IRIX 7

**MIPS Indy (IP22, MIPS III, big-endian, small TLB, limited RAM).** The design avoids every avoidable source of overhead: no microkernel IPC, no per-syscall address-space switch for native code, no unwinder, no 64-bit pointers, soft-float in the kernel, and direct-mapped kernel RAM. The MIPS-specific work (TLB refill, cache maintenance, ECOFF, o32/n32 decode) is isolated in the framework and the compat container, where it can be audited and tested on the emulator before real hardware.

**GPL-3.0.** Every recommendation avoids licence traps: smoltcp is 0BSD, `abi_stable` is MIT/Apache-2.0, rustls is Apache-2.0/ISC, Theseus and Asterinas are permissive (check versions), Mesa is MIT, and the clean-room XFS avoids the GPL-2.0-only incompatibility of porting Linux XFS. ZFS (CDDL) is explicitly excluded. The ADR-0012 gate applies to every dependency, including transitive ones.

**Clean-room.** The architecture keeps SGI-derived expression out of the published tree: the compat layer is a behavioural reimplementation driven by the oracle and captured ABI facts; XFS is reimplemented from published format descriptions; the ADR-0017 catalogue prefers specifications and papers over source. The dirty/clean split (ADR-0014) is a process control, but the design choices here (clean-room XFS, generated layout crate, oracle-driven tests) make it practical.

**Backward-only.** The compat container targets IRIX 6.5 o32/n32 semantics exactly, with no attempt to make IRIX 7 binaries run on IRIX 6.5. The native ABI is free to be capability-based precisely because forward compatibility is out of scope: there is no need to fit the native model into the legacy one.

**Monolithic Rust kernel.** The framekernel + cells architecture is monolithic in deployment (one image, one address space, function-call composition) but not in trust: the TCB is the privileged framework, and services are safe Rust. This is the only published design that simultaneously delivers monolithic performance, a small sound TCB, and a credible live-evolution story.

**Capability ABI + compat container.** One authority model (capabilities) underlies both the native and legacy paths: native programs invoke capabilities directly; the compat container translates legacy syscalls into capability invocations. This avoids two parallel security models and makes the compat layer a translation layer rather than a privilege escalation surface.

**Phase 1/Phase 2 ROM (if chosen).** The ROM is an optional bring-up vehicle, not the primary boot path. If pursued, it should use smoltcp, a signed-kernel trust model rather than a clock-dependent TLS chain, explicit endianness conversions, and the same Seeq driver as the kernel. Phase 2's LFS block device should use a block cache with HTTP Range requests and a custom read-only extent layout, with ISO 9660 as the interoperability fallback.

### K.3 Suggested decision and implementation order

1. **Freeze the ABI facts**: capture the IRIX 6.5.7m sysroot, generate the layout crate, and record syscall numbers and struct offsets with provenance. (C.2, C.5)
2. **Build the framework skeleton**: boot stub, exception vectors, TLB refill, frame allocator, `IoMem`, console. Prove the tracer bullet on the emulator. (F.6–F.8, D.2, I.1)
3. **Prove the toolchain end-to-end**: GCC backend builds the kernel; LLVM builds a parallel n32 image; both run the marker. (G.1–G.3)
4. **Implement the native core objects**: handles/rights, jobs, channels, VMAR/VMO, ports, events/futex/timer. (B.2, D.5)
5. **Bring up the compat container**: o32/n32 decoders, ELF loader, minimal libc, run a hello-world IRIX binary. (C.1–C.5)
6. **Add the first real devices**: timer, HPC3 interrupts, Seeq, SCSI, RTC. (I.5)
7. **Add the scheduler and real-time classes**: injected policy, per-CPU queues, `is_running`, FIFO/RR. (H.1–H.4)
8. **Add XFS read-only**, then journal replay, then write support. (J.1, J.5)
9. **Optional: two-phase ROM** with smoltcp and signed-kernel boot. (F.1–F.4, F.9–F.10)
10. **Identity services**: `hinv`/topology, sproc share groups, graphics/media. (J.3)

### K.4 Cross-cutting risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Framework API cannot express a service without `unsafe` | High | TCB grows, architecture claim weakens | Design APIs early; treat each exemption as an ADR |
| MIPS toolchain gaps (GCC backend, `build-std`, ECOFF) | High | Blocks bring-up | Dual backend; LLD+GNU binutils; prototype early |
| Compat layer semantic gaps (`syssgi`, STREAMS, sprocs) | High | Legacy binaries fail | Oracle-driven tests; prioritise the real binary corpus |
| TLS/clock chicken-and-egg in ROM | Medium | ROM cannot fetch kernel | Signed kernel + HTTP/TFTP; TLS only when clock exists |
| ROM size (512 KiB–1 MiB) | Medium | ROM does not fit | Feature gates; measure; drop TLS from minimal ROM |
| XFS complexity | High | Long delay | Read-only first; native FS interim |
| No IOMMU | Medium | Buggy DMA corrupts memory | Typed DMA + bounce; physical security assumption |
| Cache/TLB correctness on R4000 | High | Random corruption | Conservative cache ops; emulator tests; hardware tests |
| Clean-room provenance of ABI facts | Medium | Legal challenge | Record provenance; legal review; specifications first |
| Scheduler policy starvation | Low | System hangs | Framework watchdog; max slice |
| `abi_stable`/dependency `no_std` on MIPS | Medium | Build failure | Validate early; fall back to `repr(C)` |

### K.5 Open questions for the project to resolve

1. Is the "Physical Rust ROM" actually flash-resident on an Indy, or a boot image on disk? This determines whether Phase 1 is deployable or emulator-only.
2. What is the exact IRIX syscall error convention (flag register vs negative return), and where is `errno` stored for o32 and n32?
3. Which IRIX binaries form the compatibility corpus, and which `syssgi`/STREAMS/sproc features do they actually use?
4. Does the Indy's HPC3 provide any DMA address translation or limit registers that could substitute for an IOMMU?
5. What is the RTC/NVRAM part and battery status on the target hardware, and is a hardware fix acceptable?
6. Should the native ABI expose a POSIX-like convenience layer, and if so, how is ambient authority avoided?
7. What is the licence of `danifunker/rust-irixlibstd` and `atomchild411/rust-irix`, and which ABI facts may be reused?
8. Is XFS write support required for the first release, or is read-only plus a native FS sufficient?
9. How will the kernel be updated on a machine whose only network path is the ROM?
10. What is the acceptance test for "runs existing IRIX 6.5 binaries unchanged" — which applications, which versions, which behaviours?

---

## Appendix A. Consolidated sources

### Papers and specifications

- Peng, Tian, Junyang, Li, Chen, Jiang, Xian, Wang, Xu, Zhou, Luo, Yan, Zhang. "Asterinas: A Linux ABI-Compatible, Rust-Based Framekernel OS with a Small and Sound TCB." USENIX ATC 2025. https://arxiv.org/abs/2506.03876
- Baumann et al. "The Multikernel: A New OS Architecture for Scalable Multicore Systems." SOSP 2009. https://www.barrelfish.org/publications/barrelfish_sosp09.pdf
- Engler et al. "Exokernel: An Operating System Architecture for Application-Level Resource Management." SOSP 1995. https://pdos.csail.mit.edu/papers/exokernel-sosp95/
- Madhavapeddy et al. "Unikernels: Library Operating Systems for the Cloud." ASPLOS 2013. https://dl.acm.org/doi/10.1145/2451116.2451166
- Hunt, Larus. "Singularity: Rethinking the Software Stack." 2007. https://www.microsoft.com/en-us/research/publication/singularity-rethinking-the-software-stack/
- Klein et al. "seL4: Formal Verification of an OS Kernel." SOSP 2009. https://sel4.systems/Research/
- Liedtke. "Improving IPC by Kernel Design." SOSP 1993. https://dl.acm.org/doi/10.1145/168619.168633
- Sweeney et al. "Scalability in the XFS File System." USENIX ATC 1996. https://www.usenix.org/legacy/publications/library/proceedings/sd96/full_papers/sweeney.pdf
- Chase, Levy, Feeley, Lazowska. "Sharing and Protection in a Single-Address-Space Operating System." ACM TOCS 1994.
- Drepper. "Futexes Are Tricky." https://www.akkadia.org/drepper/futex.pdf
- CA/Browser Forum baseline requirements (certificate validity). https://cabforum.org/
- RFC 791 (IPv4), RFC 8200 (IPv6), RFC 9110 (HTTP semantics, Range requests), RFC 8446 (TLS 1.3).

### Project documentation and repositories

- Theseus OS. https://github.com/theseus-os/Theseus ; book: https://theseus-os.github.io/Theseus/book/index.html ; site: https://theseus-os.com/
- Asterinas. https://github.com/asterinas/asterinas
- seL4. https://sel4.systems/ ; manual: https://sel4.systems/Info/Docs/seL4-manual-latest.pdf ; verification: https://sel4.systems/Verification/
- Redox OS. https://redox-os.org/ ; kernel: https://gitlab.redox-os.org/redox-os/kernel ; relibc: https://gitlab.redox-os.org/redox-os/relibc
- Tock OS. https://tockos.org/ ; "Tock: A Secure Embedded Operating System for Microcontrollers," ASPLOS 2017.
- RedLeaf. "RedLeaf: Isolation and Communication in a Safe Operating System," OSDI 2020. https://www.usenix.org/conference/osdi20/presentation/narayanan-vikram
- Barrelfish. https://barrelfish.org/
- MINIX 3. https://www.minix3.org/
- Genode. https://genode.org/
- HelenOS. https://www.helenos.org/
- Fuchsia/Zircon object model. https://fuchsia.dev/ ; https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/docs/concepts/
- Linux kernel documentation (XFS, DMA API). https://docs.kernel.org/
- Philipp Oppermann, "Writing an OS in Rust." https://os.phil-opp.com/
- rust-osdev. https://rust-osdev.com/ ; https://github.com/rust-osdev
- smoltcp. https://github.com/smoltcp-rs/smoltcp
- `abi_stable`. https://crates.io/crates/abi_stable ; https://docs.rs/abi_stable
- rustls. https://github.com/rustls/rustls ; webpki: https://github.com/rustls/webpki ; embedded-tls: https://github.com/drogue-iot/embedded-tls
- Git LFS specification. https://github.com/git-lfs/git-lfs/blob/main/docs/spec.md ; API: https://github.com/git-lfs/git-lfs/tree/main/docs/api
- GitHub LFS REST API. https://docs.github.com/en/rest/repos/lfs
- FatFs. http://elm-chan.org/fsw/ff/00index_e.html
- littlefs. https://github.com/littlefs-project/littlefs
- SquashFS. https://github.com/plougher/squashfs-tools ; EROFS: https://erofs.docs.kernel.org/
- Multiboot2 specification. https://www.gnu.org/software/grub/manual/multiboot2/
- Limine bootloader. https://github.com/limine-bootloader/limine
- UEFI specifications. https://uefi.org/specifications
- `rustc_codegen_gcc`. https://github.com/rust-lang/rustc_codegen_gcc
- Rust custom targets. https://doc.rust-lang.org/rustc/targets/custom.html
- Rust platform support. https://doc.rust-lang.org/rustc/platform-support.html
- Rust reference, type layout. https://doc.rust-lang.org/reference/type-layout.html
- Rust FAQ, "Does Rust have a stable ABI?" https://prev.rust-lang.org/en-US/faq.html#does-rust-have-a-stable-abi
- Miri. https://github.com/rust-lang/miri ; Kani: https://github.com/model-checking/kani
- `cargo-call-stack`. https://github.com/japaric/cargo-call-stack
- `danifunker/rust-irixlibstd`. https://github.com/danifunker/rust-irixlibstd
- `atomchild411/rust-irix`. https://github.com/atomchild411/rust-irix
- OpenBSD sgi port. https://www.openbsd.org/sgi.html ; NetBSD sgimips: https://www.netbsd.org/ports/sgimips/ ; Linux/MIPS SGI IP22: https://www.kernel.org/
- `hwloc`. https://www.open-mpi.org/projects/hwloc/
- devicetree specification. https://www.devicetree.org/

### IRIX 7 project sources (internal)

- `README.md`, `docs/map.md`, `docs/spec/architecture.md`.
- ADR-0008 to ADR-0020 as referenced in those documents (hosted at `github.com/irix7/project/docs/adr/`).
- `targets/mips3-sgi-irix7-n32.json`, `targets/mips2-sgi-irix7-o32.json`.

---

*End of report.*

