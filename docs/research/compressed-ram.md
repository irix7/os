# Compressed RAM (CRAM) and IRIX 7

**Question.** What is Meta's CRAM, what does it actually measure, and can — or should —
IRIX 7 adopt anything like it on an SGI Indy (IP22, R4400/MIPS III, 32–256 MiB, no MMU
userspace yet, no swap)?

**Short answer.** CRAM is a Linux memory-management service for a memory device that
compresses in hardware while preserving cache-line/byte access. It is not a codec, a CPU
feature, firmware or a hypervisor. The kernel exposes the device as a private NUMA node,
keeps pages mapped read-only, and promotes a page back to DRAM on write. The headline
452× is a read-only microbenchmark measured on a DRAM-backed emulated device, not a
measurement of real compressed memory. IRIX 7 should **reject** CRAM and compressed RAM
tiers, and keep compression in the read-only EROFS root where it already belongs.

**Confidence.** Claims marked **verified** come from the LPC abstract and slide deck, the
RFC v4 patch series, or the public kernel/QEMU branches. Claims marked **(est.)** are
extrapolations from the project's own EROFS research. The daily.dev URL is a two-minute
aggregator summary that matches Hackaday's explainer, which in turn cites Phoronix and the
LPC session; it is not independent evidence. See §9 for the source chain.

**Contents.** [1. What CRAM is](#1-what-cram-is) ·
[2. How it works](#2-how-it-works) ·
[3. What the benchmarks show](#3-what-the-benchmarks-show) ·
[4. What "memory layers" means](#4-what-memory-layers-means) ·
[5. Linux-specific versus generic](#5-linux-specific-versus-generic) ·
[6. Feasibility on MIPS III](#6-feasibility-on-mips-iii) ·
[7. Assessment by category](#7-assessment-by-category) ·
[8. Recommendation and plan change](#8-recommendation-and-plan-change) ·
[9. Sources](#9-sources)

---

## 1. What CRAM is

CRAM (**Compressed RAM**) is **a Linux kernel memory-management service for a
hardware-compressed memory device**. The compression and decompression are performed by
the memory device; the kernel decides which pages live there, keeps them mapped
read-only, and moves them back to DRAM when they are written. In the RFC it is
`mm/cram.c` plus a new `N_MEMORY_PRIVATE` NUMA node state; the target hardware is a
CXL type-3 device with inline compression, and the RFC notes that the concept is not
inherently CXL-specific.

Three properties define the design:

1. **The device keeps memory semantics.** It provides cache-line/byte access to
   compressed memory, so a page can remain page-table mapped and page-cache resident. A
   read is an ordinary load; the device decompresses it on the way to the CPU. There is no
   fault and no software decompression step.
2. **The device lies about capacity.** It advertises more logical capacity than it
   physically has, betting on a compression ratio. The kernel cannot assume that every
   `struct page` it can allocate is backed by real storage. The RFC calls this the
   "capacity lie" problem.
3. **The device is read-only to the kernel.** Every write to a compressed page takes a
   protection fault and promotes that page back to DRAM before the write proceeds. CRAM is
   therefore a read-mostly tier: clean page-cache pages and cold anonymous pages, not a
   write-heavy working set.

The LPC abstract puts the distinction from zram/zswap directly:

> Unlike zswap/zram - these devices provide cacheline/byte access to compressed memory,
> their memory can remain page-table mapped and page-cache present without generate [sic]
> faults on read-access (no COW, no software decompression step etc).

And the RFC's conservative position:

> This MVP takes a conservative position: all compressed memory is mapped read-only.

**What it is not.**

- Not a compression algorithm. There is no zstd/LZ4 codec in the CRAM service; the codec
  is in the device.
- Not a CPU feature. It needs no ISA extension and no SIMD; it needs a memory device that
  can decompress a cache line on a load.
- Not firmware or a hypervisor feature. QEMU is a test vehicle, not the mechanism.
- Not zram or zswap. Those are swap-layer compressed pools; CRAM is a resident memory
  tier.
- Not upstream and not shipping. The RFC is v4 (27 patches, 54 files, 4,057 insertions);
  the LPC talk is a status report, not a merge announcement. No commercial silicon is
  demonstrated in the public material.

**Status (verified).** The RFC v4 cover letter is dated 22 February 2026; the LPC talk
was on 5 October 2026. The RFC adds 508 lines to `mm/cram.c` and 1,025 lines to the CXL
compression driver. The "~350 functional lines" in the cover letter refers to the CRAM
service logic, not the whole series. The abstract claims that all but one required feature
already exists in the kernel (or has existed at one time); that is consistent with the
split: patches 1–22 are private-node plumbing, patch 23 is `mm/cram.c`, and patches 24–27
are CXL driver examples; the CRAM service itself reuses existing reclaim, migration and
fault paths.

## 2. How it works

```text
   DRAM (N_MEMORY)                         private node (N_MEMORY_PRIVATE)
   writable, hot                           compressed, read-only,
                                           device-decompressed on read
        |   reclaim / demotion (migrate)         |
        | --------------------------------------> |
        | <-------------------------------------- |
        |   write fault (promote to DRAM)         |
        +------------- clean page cache ----------+
```

1. **Registration.** A driver registers a private node with
   `add_private_memory_driver_managed()`. The memory is hotplugged, marked
   `N_MEMORY_PRIVATE`, and excluded from normal zonelist fallback. Only explicit
   `__GFP_PRIVATE` allocations can reach it, so ordinary `malloc()` can never land there.
2. **Demotion.** Reclaim selects cold folios and migrates them to the private node. On
   arrival the PTEs are write-protected. On real hardware the device compresses them; in
   the QEMU model they are backed by DRAM and only the capacity accounting is modelled.
   Either way the kernel sees a normal `struct page`.
3. **Read.** The PTE is valid. The load goes to the device; the device returns the
   decompressed cache line. No page fault, no `memcpy`, no software decompressor. This is
   the whole point of the design.
4. **Write.** The write-protect fault calls `handle_fault`, which migrates the folio back
   to DRAM, updates the PTE, and retries the write. Reads never fault; every write to a
   compressed page does.
5. **Page cache.** Only clean pages are eligible. A dirty page must stay in DRAM, because
   write-back would otherwise have to decompress, modify and recompress behind the
   filesystem's back.
6. **Capacity control.** The device reports real utilisation through watermark
   interrupts. `watermark_boost` throttles demotion when capacity is tight; the
   "chicken bit" tells the kernel to stop allowing new allocations; ballooning resizes
   the logical node; free page reporting trims stale data. If the compression ratio is
   poor, the service stops demoting and starts evicting.
7. **Free.** `free_folio` scrubs stale data before the folio returns to the buddy
   allocator, so a later allocation cannot read a previous tenant's compressed bytes.

The design reuses Linux's existing reclaim, migration, fault and NUMA machinery. That is
why the service is small relative to the 27-patch series: most of the series is the
private-node infrastructure and the CXL driver, not the compression policy.

**Why not swap?** The deck cites Kairui Song's measurements: in the best case,
decompression is less than 10% of the cost of a swap fault. The real win is getting rid of
the fault, not making decompression faster. CRAM is an attempt to keep the page resident
and mapped so that a read never enters the swap path at all.

## 3. What the benchmarks show

**Setup (verified from the slides).** The read-only run uses a 46 GiB anonymous footprint
and 0% writes; the write runs use the same 36 GiB of physical memory at 2–20% writes. Every tier gets the
same 36 GiB of physical memory; the DRAM ceiling gets 54 GiB. The LPC abstract says
TAOBench and FIO. The slides report "operations per second" on a log scale; the deck does
not define an operation, but the 46 GiB anon footprint and the page-fault accounting imply
page-granularity accesses. Treat the numbers as page-throughput, not per-access latency.

**Read-only throughput** (millions of page-granularity operations per second):

| Workload | DRAM ceiling | CRAM | zram (zstd) | zswap (zstd) | Disk swap |
|---|---:|---:|---:|---:|---:|
| Skewed (Zipf 0.99) | 325 | 321 | 9.7 | 0.32 | 3.1 |
| Uniform random (worst case) | 498 | 489 | 1.1 | 0.043 | 0.36 |

CRAM is 98.2–98.8% of the DRAM ceiling. The 452× headline is CRAM versus zram in the
uniform-random read-only case; the Zipf case is 33×. This is the strongest result, and it
is exactly what the design predicts: no read faults and no software decompression.

**Write scaling** (CRAM speed-up over zram, same 36 GiB physical):

| Writes | 0% | 2% | 5% | 10% | 20% |
|---|---:|---:|---:|---:|---:|
| Uniform random | 452× | 37× | 12× | 8.2× | 5.4× |
| Skewed (Zipf) | 33× | 12× | 8.8× | zram OOM | zram OOM |

At 20% writes, CRAM is 14×/16× disk swap (Zipf/random). Every write to CRAM faults and
copies the page back to DRAM; reads never fault. zram was OOM-killed at 10% and 20% writes
in the Zipf run (the deck does not report the compression ratio).

**Faults and stalls:**

| Tier | Swap-in faults | Direct reclaim stalls | Time fully stalled | M ops/s (Zipf/random) |
|---|---:|---:|---:|---:|
| CRAM, 0% writes | 0 | 0.4–0.5 K | 3 s | 321 / 489 |
| CRAM, 20% writes | 0 | 404–558 K | 35–37 s | 36 / 4.9 |
| zram | 31–35 M | 571–684 K | 88–93 s | 8.9–9.7 / 0.90–1.08 |
| zswap | 1.7–2.2 M | 66–78 K | 123–125 s | 0.32–0.37 / 0.043–0.046 |
| Disk swap | 6.8–7.7 M | 24–44 K | 25–32 s | 2.6–3.1 / 0.31–0.36 |

The zero swap-in faults are the mechanism: CRAM pages stay mapped, so reads never enter
reclaim. At 20% writes CRAM still has 404–558 K direct reclaim stalls and is fully stalled
for 35–37 s, so it is not stall-free; it trades swap-in faults for promotion faults and
reclaim work.

**Memory savings.** 46 GiB logical into 36 GiB physical is **1.28×** in the model. The
deck's stated global target ratio is **~2.5×, "not worked on yet"**. The slides report no
measured compression ratio, compressed-byte count, CPU utilisation, per-access latency or
bandwidth in GB/s.

**The caveats that matter more than the headline:**

- The deck says the benchmark is **"all DRAM-backed to isolate fault cost"**. The public
  QEMU device (`cxl-ct3`) is a CXL type-3 model backed by a file/DRAM region; it handles
  sanitize/zero media operations and watermark interrupts, and its `ct3-ratio` property is
  declared but unused. In the public branches, neither the QEMU device nor the Linux CXL
  driver contains a compression codec. The 321/489 M ops/s read numbers therefore do not
  include decompression; they are an upper bound on read throughput for a real device.
- The 452× is a **read-only, uniform-random** microbenchmark. At 20% writes it is 5.4×;
  at 10–20% Zipf writes the zram baseline is OOM-killed. It is not a general application
  speed-up.
- The deck uses a **log scale** and warns "Log Scale!". Small absolute differences look
  large.
- The deck marks the zswap results **"not entirely valid"** because of an unrelated ~90 ms
  reclaim stall on that setup.
- The 46 GiB/36 GiB accounting is not explained in the slides. Treat 1.28× as a model
  parameter, not a measured compression ratio.
- No commercial hardware is demonstrated in the public material. The open question is
  whether hardware compression appears in real machines at all.

In other words, the benchmark measures the value of **keeping pages mapped** (no swap-in
fault, no software decompression), not the speed of a hardware decompressor. That is a
real and interesting result; it is not evidence that compression is free.

| Claim | Evidence status |
|---|---|
| CRAM is a Linux service for a hardware-compressed memory device | **Verified**: LPC abstract, RFC v4, `mm/cram.c` |
| Reads avoid faults and run at ~DRAM throughput | **Verified for the DRAM-backed model**; not verified on real compressed hardware |
| 452× faster than zram | **Verified as a read-only, uniform-random benchmark**; 5.4× at 20% writes; not a general speed-up |
| 46 GiB into 36 GiB (1.28×) | **Model parameter** in the deck; no measured compression ratio |
| ~2.5× global target | **Stated target**, "not worked on yet" |
| CPU, latency and bandwidth overheads | **Not reported**; the deck measures throughput and stalls only |
| Commercial hardware exists | **Not verified**; no commercial silicon in the public material |

## 4. What "memory layers" means

The article uses "memory layers" in two different senses, and conflating them is the main
source of hype.

**The explainer sense.** The Hackaday/daily.dev piece walks through the general hierarchy:
CPU caches and coherence (MESI), virtual memory and the TLB, NUMA, page faults and swap,
the filesystem page cache, and the SSD's flash translation layer. Each layer hides latency
or capacity from the layer above. This is a good primer, but it is not specific to CRAM.

**The CRAM sense.** CRAM inserts a **compressed memory tier beside DRAM**, not below it as
a block device:

| Layer | What it hides | CRAM's interaction |
|---|---|---|
| CPU caches / coherence | DRAM latency; line granularity | The device returns a decompressed cache line; the coherent link must support normal loads. The CPU cache sees ordinary lines. |
| MMU / TLB / page table | Virtual-to-physical mapping; protection | Pages stay mapped. The PTE is read-only. A write fault promotes the folio and changes the PTE; reads add no TLB pressure beyond normal. |
| NUMA / memory tiers | Remote or slower memory | The compressed device is a private NUMA node. Reclaim/demotion move cold pages there; NUMA balancing and hotness decide what is cold. |
| Page cache | Disk latency | Only clean pages are eligible. Dirty pages stay in DRAM. |
| Swap / block | Memory capacity | CRAM deliberately bypasses swap. zram and zswap live in this layer. NVMe is below swap, not part of CRAM. |
| Interconnect / device | Capacity and bandwidth | CXL provides the load/store path; the device compresses. A block device cannot provide this semantics. |

So "DRAM/CXL/NVMe tiers" means: DRAM is the hot writable tier, CXL is the transport and
device that provides the compressed tier, and NVMe is the cold swap tier below. CRAM does
not turn NVMe into memory. "Compressed memory pools" means zram (a compressed block device
in RAM) and zswap (a compressed cache in front of a swap device); CRAM is a compressed
NUMA tier that stays mapped. "Cache/TLB implications" means the device must decompress at
cache-line granularity, the PTE stays valid on read, and only write promotion perturbs the
page table.

On the Indy, only the first two layers exist in hardware (caches and MMU/TLB); there is one
memory controller, no NUMA, no CXL and no NVMe. The only lower tier is EROFS over R2/SCSI.
"Memory layers" collapses to DRAM plus a read-only file cache; there is no place for a
compressed NUMA tier to sit.

## 5. Linux-specific versus generic

| Element | Linux-specific? | Generic? |
|---|---|---|
| `N_MEMORY_PRIVATE`, `__GFP_PRIVATE`, zonelists, cpusets | Yes | No |
| Reclaim/demotion, NUMA balancing, mempolicy, memory hotplug, `struct page`/folio, kswapd, PSI, OOM killer | Yes | The concepts exist elsewhere, but this implementation is Linux's |
| `mm/cram.c` policy: read-only tier, promote on write, clean cache only, balloon, chicken bit | Implementation is Linux | The policy is OS-independent |
| CXL type-3 driver, media operations, watermark MSI-X | Yes | Hardware-specific |
| "Fault cost dominates decompression" | No | Yes |
| "Compressed memory as a NUMA tier" | Linux mechanism | Yes, in principle |
| zram/zswap | Yes | Compressed swap/pool is generic |

The reusable idea is not the code. It is the observation that **a compressed memory tier
should avoid the swap fault path**, and that a read-only mapping with promotion on write is
a safe way to do it. IRIX 7 can apply that idea only where it has cold, read-only data:
the EROFS root. It cannot reuse the Linux implementation without rebuilding the Linux MM.

## 6. Feasibility on MIPS III

| Resource | Indy | Consequence for compression |
|---|---|---|
| CPU | R4400SC at 75 MHz (100 MHz variants); in-order, no SIMD | LZ4 decode 9–19 MB/s **(est.)**; compression is slower still |
| D-cache | 16 KiB, 16-byte lines, direct-mapped | A 4 KiB decompression touches 25% of the cache; conflict misses are likely |
| TLB | 48-entry JTLB, 4 KiB–16 MiB pages | Promotion/demotion changes PTEs and causes refills |
| Memory | 32–256 MiB, 266 MB/s peak | A 1.28× saving is ~7 MiB at 32 MiB; 2× is ~16 MiB |
| I/O | HPC3 30 MB/s, SCSI 10 MB/s, Ethernet 1.1 MB/s TCP | Disk/network tiers are slow; no CXL or NVMe |
| Expansion | GIO32/GIO64, 32-bit, no hotplug, not coherent | No cache-coherent compressed-memory device |

**The software cost.** The project's EROFS research estimates LZ4 decode at 4–8 cycles per
byte on an in-order, 1-IPC, 16 KiB-cache machine: **9–19 MB/s at 75 MHz**, or
**0.2–0.45 ms per 4 KiB page** **(est.)**. Compression is slower than decompression, so a
write to a compressed page costs at least a decode plus an encode, plus the fault and the
copy. A 4 KiB page transfer at the memory controller's 266 MB/s peak is ~15 µs by itself
(a copy is at least twice that); the codec dominates.

**The cache and TLB cost.** The 16 KiB D-cache is the binding constraint. One 4 KiB
decompression evicts a quarter of it, and a burst of faults evicts the working set. The
48-entry TLB adds refill cost for every promotion or demotion. The 75 MHz in-order core
cannot hide either. This is the opposite of the project's stated priority: predictable
performance on 1990s hardware.

**No SIMD is not the blocker.** LZ4 is byte/integer-oriented and runs on MIPS III. The
blockers are the clock, the 16 KiB cache, the 48-entry TLB and the absence of a
compression device. Big-endian is also not a blocker: LZ4 and zstd are byte-oriented, and
the EROFS research shows byte-swapping is noise compared with decompression.

**Could an accelerator help?** The Indy has no memory-side compression, no CXL and no
SIMD. A GIO FPGA compressor would need the CPU to copy pages to the card and back over a
30 MB/s HPC3, with cache maintenance; a 4 KiB round trip is already ~0.27 ms at 30 MB/s
before compression, and GIO is not cache-coherent. It would be slower and far more complex
than software LZ4. The Newport blitter is a 2D graphics engine, not a codec. Reject.

## 7. Assessment by category

| Category | What it would buy | Fit on the Indy | Verdict |
|---|---|---|---|
| (a) Compressed swap / zram-style | 1.3–2× effective RAM; fewer disk reads | Feasible after MMU/swap, but CPU/cache/TLB cost is high and v1 has no swap | **Reject for v1**; no spike now |
| (b) Compressed page cache / root | Smaller root image; less network I/O | Already planned as EROFS LZ4; the network is the bottleneck | **Adopt** (already in M4) |
| (c) Hardware/accelerator compression | Near-DRAM compressed reads | No cache-coherent compressed-memory device; GIO is not coherent | **Reject** |
| (d) Tiered memory (CXL/NVMe) | Larger memory; hot/cold placement | No NUMA, CXL, NVMe or hotplug; the only lower tier is 10 MB/s SCSI or 1.1 MB/s network | **Reject** |

**(a) Compressed swap / zram-style.** A zram read fault costs a decompress (0.2–0.45 ms
estimated) plus the fault and the copy; a write costs a compress (slower) plus the same. On
a 75 MHz machine that is a visible, variable pause for interactive work, and it competes
with the 16 KiB cache. The OS is also early: there is no MMU userspace, no swap and no
general page-cache reclaim policy beyond the planned block-cache LRU, so a compressed RAM
tier would be built on machinery that does not exist yet. The project already chooses no swap in v1 and an OOM killer, and
R12's mitigation is a 64 MiB floor, lite mode and lazy loading. Compression is not a
substitute for that: a 1.28× saving on 32 MiB is ~7 MiB, which will not turn a 32 MiB Indy
into a full-stack machine. If 32 MiB ever becomes a hard requirement after MMU and swap
exist, a capped LZ4 pool with eviction (not corruption) on incompressible pages would be
the least-bad experiment. That is a future spike, not a current milestone.

**(b) Compressed page cache / compressed root.** This is the one compression that pays.
The data is cold, read-only and network-bound; the CPU cost is paid once per block, not on
every cache hit. The project already plans EROFS in M4, with LZ4 in §11.2. The network is
the bottleneck: 1.1 MB/s TCP versus 9–19 MB/s LZ4 decode; a 64 KiB fetch takes ~60 ms at
1.1 MB/s, while decoding a 4 KiB block takes 0.2–0.45 ms. Keep the page cache
**decompressed**; a cache hit should not become a decompression event on a 75 MHz CPU.

**(c) Hardware/accelerator compression.** CRAM's kernel policy could be reused with a
software compressor, but then it is zram with a read-only constraint and extra migration
machinery. The hardware is the point. The Indy has none, and emulating it with a GIO card
would be slower than software. Reject.

**(d) Tiered memory (CXL/NVMe).** Tiering is useful when tiers differ in latency and
bandwidth and the OS can move pages profitably. On the Indy the only lower tier is 10 MB/s
SCSI or 1.1 MB/s network; moving a page there is swap, which v1 rejects. There is no NUMA
topology to manage. The existing DRAM plus EROFS page cache is the only useful hierarchy.
Reject.

**What is worth stealing from CRAM.** Two design lessons, not code:

1. If IRIX 7 ever adds a compressed page cache, keep pages mapped read-only and promote on
   write instead of modelling compression as a swap device. The fault is the expensive
   part, not the codec.
2. Any compressed pool must have explicit backpressure and a capacity ceiling. CRAM's
   "capacity lie" is a warning: a pool that promises more than it can back will corrupt
   data or OOM when the workload is incompressible.

## 8. Recommendation and plan change

**Recommendation: reject.** Do not adopt CRAM, zram/zswap, an in-RAM compressed page cache
or tiered memory for IRIX 7. Keep EROFS LZ4 for the read-only root, which is the only place
where compression buys more than it costs. Do not schedule a compressed-RAM prototype:
M2's TLB/frame allocator and M5's MMU work are on the critical path; M9's capability ABI
follows, and M4 already delivers the useful compression.

The smallest concrete plan change is **one row in `docs/spec/architecture.md` §16,
"Alternatives rejected"**:

```markdown
| **CRAM-style compressed RAM, zram/zswap, tiered memory** | The Indy has no cache-coherent memory-side compression, CXL or NUMA. Software compression costs ~0.2–0.45 ms per 4 KiB page at 75 MHz (LZ4 decode, estimated), thrashes a 16 KiB direct-mapped D-cache and adds TLB/PTE churn for a ~1.3–2× capacity gain. Compression stays in the read-only EROFS root (§11.2); see [`../research/compressed-ram.md`](../research/compressed-ram.md). |
```

That row records the decision without changing a milestone. If the maintainer prefers the
memory section to be self-contained, the same sentence can go in §7.6 after "No swap in
v1"; §16 is the canonical rejected-alternatives table, so it is the better single home.
Adding this note to `docs/research/README.md` and §18.2 is optional discoverability, not a
decision.

**No milestone change.** M4 already includes EROFS; M2 already includes the TLB and frame
allocator; M5 already includes the MMU work, and M9's capability ABI is separate. A
compressed-RAM spike would distract from boot correctness and IRIX 6.5 compatibility,
which are the project's priorities.

**What would change the verdict:**

- A cache-coherent memory-side compression device for GIO or PCI with load/store
  semantics and enough bandwidth. This is very unlikely for the Indy.
- A hard 32 MiB requirement that cannot be met by lean design and the 64 MiB floor, plus a
  measured LZ4 implementation substantially faster than 9–19 MB/s on an R4400.
- A swap implementation and a workload study showing that the compression saves more
  latency than it costs, including the cache and TLB effects.
- Evidence that CRAM's real hardware meets the 2.5× target and that the kernel mechanism
  is upstream. Neither is true today.

## 9. Sources

**Primary.**

- LPC 2026 session *A Compressed RAM Service*, Gregory Price (Meta), 5 October 2026:
  abstract, slides and video. <https://lpc.events/event/20/contributions/2424>. The slide
  deck is the source of the benchmark tables; slides 4, 10, 11 and 13 are quoted above.
  No Meta engineering blog post was found; the LPC session and the RFC are the primary
  sources.
- RFC v4 patch series, *[LSF/MM/BPF TOPIC][RFC PATCH v4 00/27] Private Memory Nodes (w/
  Compressed RAM)*, 22 February 2026:
  <https://lwn.net/Articles/1059885> and
  <https://patchew.org/linux/20260222084842.1824063-1-gourry@gourry.net/>. The cover letter
  supplies the capacity-lie, read-only MVP and ~350-line service statements; slide 10
  supplies the 2.5× target.
- Public kernel branch: `mm/cram.c`, `include/linux/cram.h`,
  `drivers/cxl/type3_drivers/cxl_compression/compression.c`:
  <https://github.com/gourryinverse/linux/tree/private_compression_v3>.
- Public QEMU device: `hw/cxl/compressed_type3/ct3.c`, `ct3.h` and the watermark script:
  <https://github.com/gourryinverse/qemu/tree/compressed_cxl_clean>. The model has no
  compression codec; `ct3-ratio` is declared but unused.
- Linux documentation for the comparison: zram
  <https://docs.kernel.org/admin-guide/blockdev/zram.html> and zswap
  <https://docs.kernel.org/mm/zswap.html>.
- LWN, *In-kernel memory compression* (2013): <https://lwn.net/Articles/545244>. Background
  on why zram/zswap use the swap layer.

**Secondary.**

- Phoronix, *Meta Developing Compressed RAM "CRAM" For Linux: Better Than ZRAM & Zswap*,
  6 October 2026: <https://www.phoronix.com/news/Linux-CRAM-Compressed-RAM>.
- Tom's Hardware, *New Linux tech compresses memory in RAM, as RAM, for 452x speedup*,
  7 October 2026. The author states they worked from the slides and were not at the talk.
- Hackaday, *Your Computer Memory Is A Lie*, 12 October 2026:
  <https://hackaday.com/2026/10/12/your-computer-memory-is-a-lie/>. The source of the
  "memory layers" explainer.

**Aggregator.**

- daily.dev, *Meta's CRAM: Compressed RAM for Linux, and the Memory Layers It Sits On*,
  12 October 2026:
  <https://daily.dev/posts/meta-s-cram-compressed-ram-for-linux-and-the-memory-layers-it-sits-on-whwqaskwy>.
  A two-minute summary; it adds no independent evidence.

**Project sources.**

- `docs/spec/architecture.md` §7.2 (address space/TLB), §7.3 (huge pages/cache colouring),
  §7.6 (allocation, "No swap in v1"), §9.3–§9.4 (hardware inventory), §11.2 (EROFS),
  §16 (alternatives rejected).
- `docs/research/erofs-big-endian-research.md` §5.4 (LZ4/DEFLATE/LZMA/zstd decode rates)
  and §5.5 (10 Mbit link).
- `docs/research/indy-hardware.md` §2 (CPU/caches/TLB) and the memory-controller section
  (266 MB/s peak).
- `docs/plan/risks.md` R12; `docs/plan/milestones.md` M2/M4; `docs/plan/hardware-lab.md`
  §10 (32 MiB is below the full-stack floor; 64 MiB is the minimum candidate).
