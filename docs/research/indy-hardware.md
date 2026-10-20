# SGI Indy (IP22 / “Guinness”) hardware reference

**Purpose.** A subsystem-by-subsystem hardware description of the SGI Indy, written for
someone bringing up a bare-metal operating system in Rust. For every block it gives the
register/address map, the programming sequence, what software must do to use the block
*fully*, and the hard limits (bandwidth, DMA addressing restrictions, byte order,
alignment, known silicon bugs). Documented behaviour is separated from driver workarounds
and from emulator-derived guesses throughout; the residual unknowns are collected in
[§18](#18-unresolved-and-uncertain).

**Machine identity.** The Indy mainboard is silkscreened **IP24**, but the firmware and
all system software identify it as **IP22**; SGI’s code name is **Guinness**. Indigo2 is
**FullHouse** and shares the IP22 architecture but differs in IOC revision, EISA, and
graphics. Every register in this document is the Indy variant unless stated otherwise;
where the two machines differ the IOC2 specification has a dedicated section [S3 §6.0].

---

## 0. Conventions and source hierarchy

* Addresses are **physical** addresses as seen through KSEG1 (`0xA000_0000 + phys`) or via
  the TLB. All peripherals live in the upper 512 MB of the 32-bit physical space.
* “Word” means 32 bits, “doubleword” 64 bits. The machine is **big-endian** in its native
  configuration; nearly every DMA engine has a per-channel endian bit, and getting it
  wrong produces silently byte-swapped data rather than a bus error.
* Register offsets are byte offsets. Devices that expose only 8-bit registers on a 32-bit
  bus generally place them in **byte lane 3** of each word (address `(reg << 2) + 3`), a
  pattern that repeats in the RTC, the Z85C30, the 82C42, and the WD33C93.
* Source tags `[S1]`…`[S21]` are resolved in [§17](#17-sources). Page references are to
  the local copies listed there.
* **Priority of evidence:** SGI chip specifications and manufacturer datasheets define
  semantics; Linux and NetBSD sources are treated as *executable evidence* of the
  sequences that actually work on hardware, and are cited as such. Where the two
  disagree the disagreement is called out rather than averaged.

---

## 1. System overview and physical address map

```
0x0000_0000  KUSEG (user, TLB mapped)
0x0800_0000  main memory, segment 0           (up to 256 MB, wraps at 0x1800_0000)
0x2000_0000  main memory, segment 1           (up to 128 MB)
0x1F00_0000  GIO64 graphics slot window
0x1F08_0000    Vino (video capture, GIO64 master)          [S12 §2.1]
0x1F0F_0000    graphics slot base: Newport / REX3 (XL, XZ)
0x1F40_0000  GIO expansion slot 0 (GIO32)
0x1F60_0000  GIO expansion slot 1 (GIO32)
0x1FA0_0000  memory controller (MC)                        [S18 mc.h: `SGIMC_BASE`]
0x1FB0_0000  second HPC3 (Indigo2 only in practice)
0x1FB8_0000  HPC3 #0                                        [S1 §3.0][S18 hpc3.h]
0x1FB9_0000    PBUS DMA channels 0..7 (0x2000 apart)
0x1FBC_4000    WD33C93 SCSI controller, channel 0
0x1FBC_C000    WD33C93 SCSI controller, channel 1
0x1FBD_4000    Seeq 8003 Ethernet
0x1FBD_8000    HAL2 audio
0x1FBD_9000    ISDN (HSCX) register window, 0x1FBD9000–0x1FBD97BB   [S3 §6.0]
0x1FBD_9800    IOC2 (serial, parallel, kbd/mouse, INT3, timers, panel)
0x1FBE_0000    DS1386 RTC + 8 KB battery-backed RAM
0x1FC0_0000  PROM (32-bit ARCS firmware)
0x1FE0_0000  Flash, 0x1FE00000–0x1FFFFFFF                        [S3 §6.0]
0xA000_1000  ARCS parameter block (KSEG1 view of a low physical page)
```

Two facts dominate bring-up. First, **there is no memory-mapped device below 0x1F00_0000
except RAM and the PROM**, so a kernel can map the whole I/O world with a handful of TLB
entries or, more simply, run everything through KSEG1 while the TLB is still being
initialised. Second, **all I/O is big-endian by default and the CPU is big-endian**, so
only the DMA engines’ endian bits (and the byte-lane-3 register convention) need attention.

The system clock tree (Indy): the SysAD clock is 50 MHz on the slower R4400 models up to
75–100 MHz on the R4600/R5000 models, and the GIO clock is 25 or 33 MHz. The 8254 inside the
IOC counts at **1 MHz**; the CP0 Count/Compare pair runs at half the CPU core clock. There is
no single “bus clock” register — software must derive timing from the 8254 or from ARCS.

---

## 2. CPU, caches and TLB

Indy shipped with four CPU options, all MIPS III except the R5000 (MIPS IV):

| Part | I/D cache | Line | Organisation | TLB | Notes |
|---|---|---|---|---|---|
| R4000SC/PC | 8 KB / 8 KB | 16 B | direct-mapped, write-back D | 48-entry JTLB (2 pages/entry), 2-entry ITLB | 50 MHz typical in Indy |
| R4400SC/PC | 16 KB / 16 KB | 16 B | direct-mapped, write-back D | 48-entry JTLB, 2-entry ITLB | 50/75 MHz, external L2 |
| R4600PC/SC | 16 KB / 16 KB | 32 B | **2-way** set-associative | 48-entry TLB (96 pages), 2-entry ITLB | 100 MHz, no on-chip L2; SC has external L2 |
| R5000SC/PC | 32 KB / 32 KB | 32 B | 2-way | 48-entry JTLB | MIPS IV, dual-issue, on-chip L2 controller (512 KB/1 MB/2 MB) |

Sources: R4000 user’s manual [S6 ch. 4, 5]; IDT R4600 “ORION” manual [S7]; R5000 product
brief [S21].

**What software must do.**

1. **Enable caches only after the caches are known-good.** The R4000 family powers up with
   caches disabled and in an undefined state. `Config.K0` selects the kseg0 cache
   attribute; `Status.CU`, `Status.IM`, and `Status.IE` gate co-processor 0 and interrupts.
2. **Data cache is write-back.** Before handing a buffer to any DMA engine, or before
   reading a buffer a DMA engine wrote, software must flush/invalidate by `Hit_Writeback_Invalidate_D`
   over the buffer’s address range (index-based, since these caches are physically indexed
   and the line size is 16 B on R4000/R4400 and 32 B on R4600/R5000). A `sync` instruction
   is required after the cache ops and before the doorbell write to the device. Getting
   this wrong is the single most common source of “the disk read random garbage” on IP22.
3. **The instruction cache is not coherent with the data cache.** After writing code,
   flush D over the range, then `cache 0x10` (I-cache invalidate) and `sync`.
4. **TLB.** 48 JTLB entries, each mapping an **even/odd page pair**; `PageMask` selects
   page size from 4 KB to 16 MB in powers of four. `Wired` reserves entries from index 0;
   `Random` decrements on TLB miss. For a kernel that maps RAM with 16 MB pages and I/O
   with 4 KB or 16 MB pages, a handful of wired entries suffices. `EntryHi.ASID` is 8 bits
   on R4000/R4400.
5. **Count/Compare is the only usable periodic timer** (see [§14](#14-interrupts-timers-and-timebase)).
   `Count` increments every second core clock cycle on R4000/R4400; on R4600/R5000 it also
   increments at half the core clock. Writing `Compare` clears the timer interrupt
   (`Cause.IP7`) as a side effect — there is no separate acknowledge.
6. **R4600/R5000 differences that matter to a low-level kernel:** 32-byte cache lines
   (double the flush loop granularity), 2-way associativity (so `Hit_*` ops still work but
   index-based loops must cover both ways — use the `Hit` variants which compare the tag),
   and on R5000 the `Config` register’s L2 fields must be programmed to match the soldered
   L2 size before enabling it.

**Hard limits.**

* 32-bit physical addressing: 4 GB total, of which the upper half is I/O. A DMA descriptor
  can only carry a 32-bit physical pointer.
* KSEG0/KSEG1 map only the low 512 MB (0x0000_0000–0x1FFF_FFFF). All devices fit, but RAM
  above 256 MB does **not** exist on Indy (see §3), so the 512 MB `AliasBig` bit in the MC
  is irrelevant here.
* The R4000 has no `eret`-visible shadow registers; the exception vector layout is the
  classic MIPS one with the `Status.BEV` offset at 0x1FC0_0200 (which lands *inside the
  PROM* — a useful detail when writing early trap handlers: with BEV set, exceptions go to
  the PROM, not to your kernel).
* R4000SC/PC errata around `lwc1`/`swc1` and partial-word stores exist; for a Rust kernel
  the practical consequences are covered by not depending on unaligned FP loads.

---

## 3. Memory controller (MC) and RAM

The MC is a single ASIC at **0x1FA0_0000** (`SGIMC_BASE`), 4 KB of registers. [S18 mc.h]
It drives two banks of four 72-pin SIMM sockets.

**Population rules.** 4–32 MB, 36-bit parity, fast-page-mode DRAM, 60 ns or 70 ns, all
four SIMMs in a bank identical. Two banks → **maximum 256 MB** (8 × 32 MB). Parity is
per-byte and errors are logged, not corrected. [S2 §2; S18 mc.h]

**Addressing.** Bank 0 appears at `0x0800_0000`, bank 1 immediately above it. A second
alias of the first 128 MB appears at `0x2000_0000` (segment 1). SGI’s memory map lets a
256 MB machine occupy `0x0800_0000–0x17FF_FFFF`, with `0x2000_0000` providing a
non-cacheable-ish window used by some firmware paths. Software should size RAM from the
ARCS memory descriptor list rather than probing, because probing a nonexistent bank on
IP22 does not bus-error cleanly.

**Key registers** (`struct sgimc_regs`, [S18 mc.h]); offsets in bytes from 0x1FA0_0000:

| Offset | Name | Function |
|---|---|---|
| 0x004 | `cpuctrl0` | refresh enable, parity enables, watchdog, warm reset, endian, snoop |
| 0x00C | `cpuctrl1` | GIO timeout, per-slot fixed/little endian (HPC, EXP0, EXP1) |
| 0x014 | `watchdogt` | read-only watchdog counter; **any write clears it** |
| 0x01C | `systemid` | MC revision (bits 3:0), EISA present (bit 4) |
| 0x02C | `divider` | RPSS (100 ns) counter divisor |
| 0x034 | `eeprom` | bit-banged serial EEPROM (Indigo2 environment) |
| 0x044 | `rcntpre` | refresh counter preload |
| 0x04C | `rcounter` | refresh counter readback |
| 0x084 | `giopar` | GIO64 width/attributes per slot (see §12) |
| 0x08C | `cputp` | CPU bus arbitration period |
| 0x09C | `lbursttp` | long-burst time period |
| 0x0C4 | `mconfig0` | bank 0 base/size/valid |
| 0x0CC | `mconfig1` | bank 1 base/size/valid |
| 0x0D4 | `cmacc` | CPU memory access config |
| 0x0DC | `gmacc` | GIO memory access config |
| 0x0E4/0x0EC | `cerr`/`cstat` | CPU-side parity error address and status |
| 0x0F4/0x0FC | `gerr`/`gstat` | GIO-side error address and status |
| 0x104 | `syssembit` | system semaphore (unibit) |
| 0x10C / 0x114 | `mlock` / `elock` | GIO memory lock, EISA lock |
| 0x154 | `gio_dma_trans` | GIO DMA address mask/translation |
| 0x15C | `gio_dma_sbits` | GIO DMA address substitution |
| 0x164 | `dma_intr_cause` | which DMA channel caused the interrupt |
| 0x16C | `dma_ctrl` | master DMA control |
| 0x184…0x1BC | `dtlb_hi0..3`, `dtlb_lo0..3` | four DMA TLB entries (see §12.2) |
| 0x100C | `rpsscounter` | free-running 100 ns counter |
| 0x1014…0x1064 | `maddronly`, `dmasz`, `ssize`, `gmaddronly`, `dmaddnpgo`, `dmamode`, `dmaccount`, `dmastart`, `dmarunning`, `maddrdefstart` | VDMA channel |

The register file therefore spans two 4 KB pages at 0x1FA0_0000 and 0x1FA0_1000.

**`mconfig` encoding.** Bits 7:0 give the bank base address in 32 MB units, bits 12:8 the
RAM size mask, bit 13 `BVALID`, bit 14 `SBANKS` (two sub-banks, i.e. interleaved). Software
that re-programs `mconfig` must first set `cpuctrl0.SYSINIT`, program both banks, then clear
it; getting this wrong hangs the machine with no bus error. In practice a kernel should
**read** these registers (ARCS has already programmed them) and never write them unless
implementing suspend/resume.

**Bandwidth.** The MC interleaves the two banks and can sustain roughly **266 MB/s** peak
for CPU accesses (64-bit SysAD at 66 MHz effective). Refresh steals a few percent.
Hardware semaphores (`syssembit`, `mlock`, `elock`) provide atomic test-and-set for
multi-processor-style locking even on a uniprocessor, and the MC also contains the **RPSS**
free-running 100 ns counter used by IRIX for profiling.

**Hard limits / traps.**

* **Parity errors are fatal-ish.** A CPU read parity error raises `SGI_BUSERR_IRQ`
  (CPU IP6). The handler must read `cstat`/`cerr`, and there is no way to correct the data;
  the usual policy is to log and kill the process, or panic.
* **The watchdog is armed by software** (`cpuctrl0.WDOG`) and cleared by reading `watchdogt`?
  No — reading returns the count, **writing any value clears it**. A kernel that enables
  the watchdog and then takes a long time with interrupts disabled will reset silently.
  Recommend leaving it disabled until there is a periodic service routine.
* Warm-reset bits (`WRESETPMEM`, `WRESETDMEM`) preserve or destroy memory contents
  respectively; they are the only clean way to reboot without a full POST.
* The MC generates bad parity on demand (`CMEMBADPAR`) — genuinely useful for testing a
  parity handler without failing hardware.

---

## 4. HPC3 — the peripheral hub

**Base 0x1FB8_0000**, 512 KB of address space, and it is the single most important block
after the CPU. It contains: a 64-bit GIO64 interface, **eight PBUS DMA channels**, **two
SCSI DMA engines**, **one Ethernet DMA engine**, the serial EEPROM interface for
Indigo2, the PBUS controller (which in turn reaches the IOC2, HAL2, ISDN, and the
parallel port), and the Dallas RTC window. [S1 §1.1, §3.0]

### 4.1 Address map inside HPC3

| Offset | Block |
|---|---|
| 0x0000_0000 | PBUS DMA channel 0: `bptr` +0x0, `dptr` +0x4, `ctrl` +0x1000 |
| 0x0000_2000 | channel 1 … one channel per 0x2000 |
| 0x0000_E000 | channel 7 |
| 0x0001_0000 | SCSI DMA engine 0 (`cbptr`, `ndptr`, `bcd` +0x1000, `ctrl` +0x1004, `gfptr`, `dfptr`, `dconfig`, `pconfig`) |
| 0x0001_2000 | SCSI DMA engine 1 |
| 0x0001_4000 | Ethernet DMA engine (RX block at +0x0000, TX block at +0x2000) |
| 0x0003_0000 | `istat0` — interrupt status, **only bits 4:0 valid** |
| 0x0003_0004 | `gio_misc` — bit 0 external timer real-time, bit 1 descriptor endian |
| 0x0003_0008 | serial EEPROM data (Indigo2) |
| 0x0003_000C | `istat1` — **only bits 9:5 valid** |
| 0x0003_0010 | `bestat` — bus error / parity status |
| 0x0004_4000 | SCSI controller 0 external registers (WD33C93 window) |
| 0x0004_C000 | SCSI controller 1 external registers |
| 0x0005_4000 | Ethernet controller external registers (Seeq window) |
| 0x0005_8000 + n·0x400 | PBUS channel *n* device registers (256 words each) |
| 0x0006_0000 | Dallas DS1386 RTC window (14 words) |
| 0x0006_0038 | battery-backed RAM (8 KB minus the RTC words) |

(The last two appear at physical `0x1FBE_0000`, consistent with `hpc3_regs.rtcregs` and
`bbram[]` in [S18 hpc3.h] and with NetBSD’s `dsclock.c`, which maps `0x1fbe0000` for IP22.)

The “external register” windows are 32 KB each and come straight from the HPC3 spec
[S1 §3.0]: HD0 `0x1FBC_0000–0x1FBC_7FFF`, HD1 `0x1FBC_8000–0x1FBC_FFFF`, ENET
`0x1FBD_0000–0x1FBD_7FFF`, PBUS `0x1FBD_8000–0x1FBD_FFFF`, battery-backed SRAM
`0x1FBE_0000–0x1FBF_FFFF`. The WD33C93 decodes at `0x1FBC_4000` inside the HD0 window; the
Seeq decodes at `0x1FBD_4000` inside the ENET window; the HAL2, ISDN and IOC2 all live in
the PBUS window at `0x1FBD_8000`, `0x1FBD_9000` and `0x1FBD_9800`.

### 4.2 The interrupt-status bug

SGI documents this as a bug and both OSes work around it: the HPC3 interrupt status is
**split across two non-adjacent registers**. `istat0` at +0x3_0000 reliably reports only
**bits 4:0**; `istat1` at +0x3_000C reliably reports only **bits 9:5**. A driver must read
both and synthesise the 10-bit status. [S1 §5.0; S19 `hpcreg.h` “HPCBUG” comment]

Bit assignment (from [S18 hpc3.h] and [S19]): bits 7:0 = PBUS devices 0–7, bit 8 = SCSI
channel 0, bit 9 = SCSI channel 1. Ethernet and SCSI DMA-completion interrupts arrive via
the PBUS/SCSI status, not as separate GIO lines.

### 4.3 PBUS DMA channels

Each channel has three registers and a per-channel timing block:

* `bptr` (+0x0000) — physical address of the current buffer (diagnostic readback).
* `dptr` (+0x0004) — physical address of the **descriptor chain**.
* `ctrl` (+0x1000) — write: start/stop control; read: `bit0` interrupt pending,
  `bit1` channel active.

Write-side control bits: `SEL` (0x2, little-endian transfer), `RCV` (0x4, direction is
receive), `FLSH` (0x8, flush on receive), `ACT` (0x10, start), `LD` (0x20, load enable),
`RT` (0x40, real-time GIO servicing), `HW` (0xFF00, FIFO high-water mark), `FB`/`FE`
(0x003F_0000 / 0x3F00_0000, FIFO begin/end pointers).

**Descriptor format** (8 bytes, 64-bit aligned recommended):

```
u32 pbuf;      /* physical buffer address */
u32 cntinfo;   /* byte count in bits 13:0 plus flags */
u32 pnext;     /* physical address of next descriptor, 0 terminates */
```

Flags: `EOX` 0x8000_0000 (last descriptor), `EOXP` 0x4000_0000 (end of packet), `XIE`
0x2000_0000 (interrupt at end of this descriptor), `XIU` 0x0100_0000 (buffer in use by
CPU), `EIPC` 0x00FF_0000 (Seeq external byte count), `ETXD` 0x0000_8000 (set by HPC after
transmit), `OWN` 0x0000_4000 (ring ownership, RX), `BCNT` 0x0000_3FFF (bytes).

**Hard limits.** The count field is **14 bits → 16 383 bytes per descriptor**; a transfer
larger than that must be split. Descriptors must be 8-byte aligned (the hardware fetches
them as doublewords). The descriptor pointer is a 32-bit physical address, so buffers must
be in the low 4 GB (trivially true) and, more importantly, must be **physically contiguous**
per descriptor. Descriptor endianness is selected globally by `gio_misc` bit 1 — pick big
endian and keep it.

PBUS channel timing is programmed through `pbus_dmacfg[8][128]` (D3/D4/D5 read/write
cycle counts, 16-bit mode, burst count, real-time, live DREQ) and
`pbus_piocfg[16][64]` for PIO. Defaults from ARCS are adequate for every device on an
Indy; only the parallel port in HP-Boise mode and ISDN need tuning.

### 4.4 Bandwidth and arbitration

HPC3’s aggregate throughput to memory is on the order of **30 MB/s**, far below the GIO64
peak of 320 MB/s — the hub is the bottleneck, not the bus. This is why Indy’s Fast SCSI,
10 Mbit Ethernet, and audio can all run concurrently without visible contention, and also
why a “fast” disk on Indy tops out around 10 MB/s.

---

## 5. SCSI — WD33C93A/B

Indy has **two** SCSI channels in the HPC3, but only channel 0 is populated on the
motherboard; channel 1 is used by Indigo2 and by some expansion boards. The controller is
a Western Digital **WD33C93A** (early) or **WD33C93B** (later, Fast SCSI-2 capable),
accessed through the HPC3 external window at **0x1FBC_4000** (channel 0) and
**0x1FBC_C000** (channel 1). [S1 §2.6.2; S17; S19 `wdsc.c`]

### 5.1 Register access

The WD33C93 is an indirect-register device: write the register number to the **address
register** (offset 0x0 in the window) then read/write the **data register** (offset 0x1),
both as bytes in lane 3 of a 32-bit word, i.e. `0x1FBC_4000` and `0x1FBC_4004`. The
“auxiliary” status register is at offset 0x2. The chip has a **12-byte FIFO** for
data-in/data-out phases.

The chip drives SCSI bus phases and handshakes autonomously once given a command block;
software supplies a 6-, 10-, or 12-byte CDB via the chip’s own registers (not via DMA), and
the **HPC3 DMA engine** moves only the data phase.

### 5.2 HPC3 SCSI DMA engine

Registers: `cbptr` (+0x0000, current buffer), `ndptr` (+0x0004, next descriptor), `bcd`
(+0x1000, byte count + flags), `ctrl` (+0x1004), `gfptr`/`dfptr` (+0x1008/0x100C,
diagnostic FIFO pointers), `dconfig` (+0x1010), `pconfig` (+0x1014).

`bcd`: bits 13:0 byte count, bit 14 `XIE` interrupt at end, bit 15 `EOX` last descriptor.
`ctrl`: bit 0 IRQ pending, bit 1 endian (0 = big), bit 2 direction, bit 3 flush FIFOs,
bit 4 active, bit 5 active-inhibits-PIO, bit 6 reset channel **and external controller**,
bit 7 parity error on the HPC↔WD interface.

`dconfig` defaults that matter: `HWAT` (high-water mark), `HW` 16-bit halfword DMA to the
controller, `SWAP` byte swap, `EPAR` parity check, `POLL`/`ERLY` DREQ polarity and timing.
`pconfig` has the analogous PIO timing plus `FUJI` (Fujitsu 86603 mode).

**Descriptor chain and the EOX bug.** Descriptors are the same 8-byte `{pbuf, cntinfo,
pnext}` triples used by PBUS. The chain must be terminated by a descriptor with **EOX set
and a byte count of zero**; the hardware otherwise walks off the end of the chain into
whatever follows. Linux additionally caps each SCSI descriptor at **8192 bytes**
(`sgiwd93.c`) even though the field allows 16 383, because of a documented HPC3 erratum
with long descriptors. Treat 8 KB as the safe limit.

### 5.3 The direction-bit discrepancy

This is worth stating carefully because it will bite anyone porting a driver:

* The HPC3 specification and **NetBSD** (`hpcreg.h`: `HPC3_SCSI_DMACTL_DIR 0x04 /* 0=dev->mem,
  1=mem->dev */`) agree that **bit 2 = 1 means memory → device** (transmit).
* **Linux** (`hpc3.h`) carries the comment `1=dev2mem`, i.e. the opposite.

Two independent primary-ish sources agree, and the NetBSD driver has been in production on
this hardware for decades, so the operating assumption for a new OS should be **1 =
memory → device**. Flagged in [§18](#18-unresolved-and-uncertain) because Linux’s comment is
the most widely copied text about this register.

### 5.4 Programming sequence (read, 512 bytes)

1. Ensure the WD33C93 has been reset (`ctrl.CRESET` pulse) and its own registers
   programmed (ID, sync transfer period, selection timeout).
2. Build one descriptor `{pbuf = phys(buf), cntinfo = 512 | XIE | EOX, pnext = 0}`.
3. Write `dptr = phys(descriptor)`, `bcd = 512 | XIE | EOX`, `ctrl = ENDIAN_BIG | DIR_READ
   | ACTIVE`.
4. Issue the SCSI command (READ(10)) through the WD33C93’s indirect registers; the chip
   asserts DREQ, the HPC3 feeds it from memory.
5. On completion the WD33C93 raises an interrupt (HPC3 `istat0` bit 8 for channel 0). Read
   `ctrl` to see `IRQ`, then clear it by writing `ctrl` with the IRQ bit cleared (the IRQ
   bit is cleared by writing 0 to it; reading it is the acknowledge).
6. Read the WD33C93 status/FIFO for residual byte count and sense data.
7. **Invalidate the D-cache** over the buffer before the CPU reads it.

### 5.5 Hard limits

* **Fast SCSI-2, 10 MB/s** maximum on the B revision; narrow (8-bit) bus, single-ended,
  on-board termination. The A revision is 5 MB/s. No Ultra, no wide.
* 7 devices per channel, ID 7 is the host. The internal disk is normally ID 1 or 6,
  CD-ROM ID 4/5; the PROM’s default boot path is `dksc(0,6,8)` — controller 0, SCSI ID 6,
  partition 8.
* 32-bit physical DMA addresses only; the WD33C93 itself only sees 24-bit addresses on the
  SCSI bus (irrelevant for the HPC3 path).
* No disconnects/reconnects parallelism worth relying on; the HPC3 has one DMA engine per
  channel and will serialise.
* Parity is checked on the HPC↔WD interface only (`ctrl.PERR`); SCSI bus parity is the
  controller’s business.

---

## 6. Ethernet — Seeq 8003

A **Seeq DQ8003** “Ethernet Data Link Controller” sits in the HPC3’s Ethernet window at
**0x1FBD_4000**, with its registers spaced four bytes apart (`reg << 2`, byte lane 0 — this
one is *not* lane 3). [S1 §2.6.3; S16; S18 `seeq.h`; S19 `if_sq.c`]

### 6.1 Register set (8 registers)

| Reg | Name | Function |
|---|---|---|
| 0–5 | station address | 6-byte MAC, latched at reset from the DS1386 NVRAM |
| 6 | RX command / status | command on write, status on read |
| 7 | TX command / status | command on write, status on read |

Collision counters and the SQE test register are reached through a bank-select bit in the
command registers; they are diagnostic and a kernel can ignore them.

### 6.2 DMA interface

Two independent HPC3 descriptor engines:

* **RX** at HPC3 +0x1_4000: `rx_cbptr`, `rx_ndptr`, `rx_bcd` (+0x1000), `rx_ctrl`
  (+0x1004), `rx_gfptr`, `rx_dfptr`, `reset` (+0x1014), `dconfig` (+0x1018),
  `pconfig` (+0x101C).
* **TX** at HPC3 +0x1_6000: same layout with `tx_*`, plus `tx_bcd` flags `ESAMP`
  (too late to add a descriptor), `XIE`, `EOP` (last byte of packet), `EOX`.

`rx_ctrl` mirrors Seeq status in bits 5:0, has `ENDIAN` (bit 8), `ACTIVE` (bit 9),
`AMASK` (bit 10), and `RBO` (bit 11, receive buffer overflow). `reset` has `CRESET` (0x1),
`CLRIRQ` (0x2), and `LBACK` (0x4, internal loopback — the only sane way to test the driver
without a network).

`dconfig` bits include `WCTRL` (write descriptors into the external controller port),
`FRXDC` (clear EOP status on RXDC — a **hardware Seeq fix**), `FEOP`/`FIRQ` (bad-packet
timeouts) and a programmable timeout value.

### 6.3 Frame format quirk

The Seeq/HPC3 receive path prepends **two pad bytes** to every frame, then the 6-byte
destination, 6-byte source, 2-byte length, payload, and finally a **status byte** appended
by the HPC3. So the Ethernet header starts at offset 2 of the buffer, and the buffer length
is `14 + payload + 1`. The `EIPC` field in the descriptor’s `cntinfo` carries the Seeq’s
external byte count. [S19 `if_sq.c`]

### 6.4 Programming sequence (transmit)

1. Build a buffer with the 14-byte header (no pad on transmit).
2. Descriptor: `pbuf = phys(frame)`, `cntinfo = len | XIE | EOP | EOX`, `pnext = 0`.
3. Write `tx_ndptr` and `tx_bcd` with the same flags, then `tx_ctrl = ACTIVE`.
4. Poll `tx_ctrl.ACTIVE` or wait for the PBUS interrupt; check `STAT4` (late collision)
   and `STAT75` (Seeq status) for errors.

### 6.5 Hard limits

* **10 Mbit/s, half duplex.** No auto-negotiation, no 100 Mbit, no full duplex.
* **AUI vs twisted pair** is selected by the IOC2 `write` register: `EPSEL` (0x04) forces
  AUI when 1 and TP when 0; `EASEL` (0x08) enables autoselect. On an Indy with no AUI
  transceiver, autoselect can hang for seconds — set it explicitly.
* The MAC address comes from NVRAM offset **0x4E8**, six bytes, big-endian order, SGI’s
  `08:00:69` OUI. A dead RTC battery yields `ff:ff:ff:ff:ff:ff`; a kernel must detect this
  and either generate a locally-administered address or refuse to bring the interface up.
* The HPC3 exposes separate 8 KB FIFO windows for receive (0x1FBA_C000–0x1FBA_DFFF) and
  transmit (0x1FBA_E000–0x1FBA_FFFF) [S1 §3.0]; the Seeq itself has only a small internal
  FIFO. If the descriptor ring underruns, the controller emits a truncated frame. Keep at
  least two transmit descriptors queued.
* Descriptors are 8-byte aligned, 16 383-byte maximum per descriptor.

---

## 7. Audio — HAL2

The **HAL2** (High-fidelity Audio Logic 2) ASIC at **0x1FBD_8000** provides 16-bit stereo
ADC/DAC, AES/EBU digital I/O, a synthesizer port, and a relay for the headphone jack. It is
a PBUS device with **indirect registers**: a 32-bit address register (`IAR`, 0x30) and four
32-bit data registers (`IDR0`–`IDR3`, 0x40/0x50/0x60/0x70). Status is at 0x10, revision at
0x20. [S18 hal2.h/hal2.c; S20 `docs_Indy_hal2.md`]

### 7.1 Register access protocol

1. Compose the 16-bit indirect address: `type` in bits 15:12, `instance` in 11:8,
   `access` (1 = read) in bit 7, `param` in bits 3:2, `index` in bits 1:0.
2. Write it to `IAR` (0x30).
3. Read or write `IDR0`…`IDR3` — a 64-bit register uses two consecutive IDRs.

Direct registers:

| Offset | Name | Notes |
|---|---|---|
| 0x10 | `ISR` | bit 0 transaction busy, bit 1 utime armed, bit 2 quad mode, bit 3 global reset (0 = reset), bit 4 codec reset (0 = reset) |
| 0x20 | `REV` | bit 15 audio absent, bits 14:12 board rev, 7:4 major chip rev, 3:0 minor |
| 0x30 | `IAR` | indirect address |
| 0x40–0x70 | `IDR0..3` | indirect data |

Useful indirect registers: `0x9104` DMA port enable (per port: synth in, AES RX, AES TX,
codec TX, codec RX), `0x9108` per-port DMA endian, `0x910C` global PBUS DMA drive,
`0x9100` relay, `0x1404/0x1408` DAC control (16/32-bit), `0x1504/0x1508` ADC control,
`0x1304` AES TX control, `0x1204` AES RX control, and three Bresenham clock generators at
`0x2104/0x2108`, `0x2204/0x2208`, `0x2304/0x2308`.

### 7.2 Clocks

The audio clock is derived from the 25 MHz PBUS clock by a **Bresenham divider**. Two
standard frequencies are programmed: 48 kHz (the default, `0x2104` series) and 44.1 kHz.
Because it is a fractional divider the *average* frequency is exact but individual sample
periods jitter by one 25 MHz tick; a kernel doing its own clock recovery should not assume
a fixed sample period.

### 7.3 Data path

* Format: **16-bit big-endian stereo PCM**, interleaved left/right, carried in 32-bit words
  on the PBUS. (The “16-bit” and “32-bit” in the register names `H2I_DAC_C1`/`C2` refer to the
  width of the *control* register, not of the samples.) Linux exposes it as
  `S16_BE`, 8 kHz–48 kHz. [S18 hal2.c]
* Mono and “quad” (two stereo pairs) modes exist; quad is selected by `ISR.QUAD_MODE`.
* The DAC/ADC are fed by **HPC3 PBUS DMA channels 0–3**; the HAL2 only raises DREQ. Each
  descriptor carries interleaved samples in big-endian order (there is a per-port endian bit
  at `0x9108` if you want little-endian).
* There is **no sample-rate conversion** and no mixing in hardware. Rate conversion, mixing,
  and volume in software are the OS’s job; the only hardware gain control is the DAC’s
  per-channel attenuation field (`H2I_C2_L_ATT_M`/`R_ATT_M`) and the analogue front-panel
  volume potentiometer, which is *not* software-readable (the panel only reports up/down
  button edges).

### 7.4 Programming sequence (playback)

1. Release reset: write `ISR` with `GLOBAL_RESET_N | CODEC_RESET_N` set.
2. Program `0x9104` to enable the codec-TX DMA port; set `0x9108` endian as desired.
3. Program the DAC control registers for 16-bit mode, unmute, and set gain.
4. Build a PBUS descriptor ring of period-sized buffers (e.g. 48 kHz × 2 ch × 4 B × 10 ms
   = 3840 bytes) with `XIE` on every Nth descriptor so the interrupt rate stays sane.
5. Start the HPC3 channel with `ACT | LD`; refill buffers from the interrupt handler.
6. To stop: clear the DMA port enable, then reset the channel.

### 7.5 Hard limits

* **No hardware mixing** — one playback stream, one capture stream (plus AES).
* The HAL2 is a **PBUS slave**; its DMA goes through the HPC3, so its 16 383-byte
  descriptor limit and 32-bit addressing apply. Total audio bandwidth is trivial
  (176.4 KB/s for 16-bit 44.1 kHz stereo) but the HPC3 has no scatter-gather across 4 KB
  pages.
* Attenuation is a small per-channel 4-bit field (roughly 1.5 dB per step); below its range
  the analogue pot takes over and software cannot see it.
* **Full-duplex is possible** (separate ADC and DAC DMA ports) but only at the same clock,
  because there is one Bresenham generator per direction but only one codec clock domain.
* Headphone **relay** (`0x9100` bit 0) must be closed explicitly or the jack is mute.

---

## 8. ISDN

Indy was sold with an optional ISDN interface. The **IOC2 contains the glue logic only**:
it adapts the Siemens **HSCX** (Extended Serial Communication Controller, PEB 2085/2086
family) to the HPC3’s PBUS DMA request/acknowledge protocol. [S3 §2.6.3, §4.6]

### 8.1 What the glue does

The HSCX has **one DMA acknowledge for both transmit and receive** on each of its two
B-channels, while the HPC3 expects separate request/acknowledge pairs. The IOC2 therefore
provides, per channel:

| Signal | Direction (from IOC2) | Meaning |
|---|---|---|
| `ISDN_DACKTx_N` | input | transmit DMA acknowledge from HPC3 |
| `ISDN_DACKRx_N` | input | receive DMA acknowledge from HPC3 |
| `ISDN_DRQTx` | input | transmit DMA request from HSCX |
| `ISDN_DRQRx` | input | receive DMA request from HSCX |
| `ISDN_DACKx_N` | output | combined acknowledge to HSCX |
| `ISDN_DRQTx_N` | output | transmit request to HPC3 (active low) |
| `ISDN_DRQRx_N` | output | receive request to HPC3 (active low) |
| `ISDN_RST` | output | reset to the HSCX, controlled by `reset` bit 3 |

The channels are enabled through the IOC2 **`dmasel`** register: bit 0 = ISDN B channel,
bit 1 = ISDN A channel, bit 2 = parallel port DMA. Bits 5:4 select the serial clock
(00 = 10 MHz, 10 = 6.67 MHz, 11 = external). [S18 ioc.h: `SGIOC_DMASEL_*`]

### 8.2 What software must do

**The register window is at 0x1FBD_9000–0x1FBD_97BB.** The IOC2 specification is unusually
blunt about the state of this block: *“Guinness has built-in ISDN. Registers are located at
0x1FBD9000-0x1FBD97BB. Since no S/W has been written for ISDN, these registers should not
get written to.”* [S3 §6.0] That is the primary source, and it is the reason no OS supports
this hardware.

To use it at all:

1. Take the HSCX out of reset: write IOC2 `reset` with bit 3 set (1 = normal, 0 = reset).
2. Enable the relevant channel in `dmasel`.
3. Program the HSCX itself through the window above. **No register map for the HSCX is
   published in any of the Indy documents available here** — the IOC2 spec covers only the
   glue, and neither Linux’s `arch/mips/sgi-ip22` nor NetBSD’s `sys/arch/sgimips` contains
   an HSCX driver. The chip is a Siemens PEB 2085/2086; its own datasheet is the only route
   to a register map, and the Indy wiring must be inferred from the glue description.
4. Handle two DMA channels per B-channel (TX and RX) plus the HSCX’s own interrupt, which
   arrives as `SGINT_ISTAT1_ISDNH` (0x04) and `ISDNI` (0x01) in the IOC2 interrupt status.

### 8.3 Status and hard limits

**Honest assessment: this subsystem is effectively undocumented and untested in any open
source OS.** No mainline driver exists, no register map is published in the sources
consulted, and the only public description is the IOC2 glue above. A bare-metal OS should
either (a) not claim ISDN support, or (b) treat it as reverse-engineering work starting from
the Siemens PEB 2085/2086 datasheet plus the glue description here.

Hard limits that *are* documented: two 64 kbit/s B-channels plus one 16 kbit/s D-channel
(the HSCX handles the D-channel itself); **no on-board NT1** — Indy’s ISDN is a
terminal-equipment interface and needs an external NT1; the physical connector is a
proprietary 8-pin modular jack, not RJ-45. There is no POTS codec and no echo cancellation
in hardware.

---

## 9. Graphics

This is the most confusing part of the Indy story because **three different graphics
subsystems** shipped under four names. All of them live in the GIO64 graphics slot window at
**0x1F00_0000** and decode the board at **0x1F0F_0000**.

| Marketing name | SGI option family | Chips | Depth | Notes |
|---|---|---|---|---|
| **XL 8-bit** | GR1-class “entry” | REX3 + RB2 + XMAP9 + VC2 (+ CMAP) | 8-bit | no Z-buffer, no geometry engine |
| **XL 24-bit** | GR2-class “express” | REX3 + RB2 + XMAP9 + VC2 | 24-bit | same pipeline, three 8-bit planes |
| **XZ (“Extreme”)** | GR3-class | Elan-derived, 4 geometry engines + Z-buffer | 24-bit | **two boards** |

“Newport” is the chipset family name (REX3/RB2/XMAP9/VC2) used by both the Indy XL boards
and the Indigo2 XL boards, not a separate product. SGI’s GR1/GR2 option codes both refer to
Newport-generation hardware at different price points; **the exact GR1↔GR2 split is not
documented in the sources available here** and is flagged in [§18](#18-unresolved-and-uncertain).

Sources: [S8] REX3, [S9] VC2, [S10] XMAP9, [S11] RB2, [S20] `docs_Indy_rex3.md`,
`docs_indy-xz-elan.md`.

### 9.1 REX3 — the drawing engine

**Base 0x1F0F_0000.** Any access to **base + 0x0800** triggers `GO`; there is no separate
command register. All coordinates are biased by **+4096** in X and Y, so the visible
1280×1024 region lives at 4096…5375 × 4096…5119, and the offscreen area extends 64 pixels
beyond. [S8 §3; S20]

Key registers (word offsets from base):

| Offset | Name | Function |
|---|---|---|
| 0x0000 | `DRAWMODE0` | 4:3 draw depth (00=4bpp, 01=8bpp, 10=12bpp, 11=24bpp), 2 logical-op select, 0 fast-clear enable |
| 0x0004 | `DRAWMODE1` | 19:17 VRAM refresh cycles per transfer, 4:3 depth, 2:0 plane enables |
| 0x0008 | `SMASK0..4` | 5×24-bit span masks (write enables per byte) |
| 0x001C | `COLORVRAM` | fast-clear colour injected during block fills |
| 0x0100 | `DCDR` | DCB data register — all VC2/XMAP9/RAMDAC traffic goes through here |
| 0x0104 | `DCDR` address/command |
| 0x0110 | `CIDMATCH` | 24-bit pixel compare value for conditional writes |
| 0x0114 | `CIDMASK` | compare mask |
| 0x0118 | `HOSTRW` | host read/write window into VRAM |
| 0x0120 | `MODE` | 0 = 8-bit host, 1 = 32-bit |
| 0x0130 | `CONFIG` | GIO32/GIO64 mode, bus width, FIFO watermarks |
| 0x0200 | `GO` | any access starts the pipeline |

**Hard limits.** 1280×1024 maximum plus 64-pixel offscreen; writes outside the physical
VRAM boundary (X < 4096 or X > 5439) are **silently culled**, which lets software send
oversized primitives without corrupting memory — but reads are *not* clipped. The command
FIFO is shallow (a few dozen entries); a driver must poll the “FIFO full” status before
pushing large batches, and the FIFO-full condition is also a GIO interrupt source.
24-bit colour is stored as three 8-bit planes, not packed RGB — a linear framebuffer read
therefore needs the XMAP9 to do the packing, or three separate plane reads.

### 9.2 RB2 — VRAM formatter

RB2 sits between REX3 and the VRAM: 24-bit write mask, 12-bit draw register, **66 MHz
maximum**, 33 MHz system clock. It performs the read-modify-write for 4/8/12-bit pixels,
the fast-clear colour fill, and the data rotation that turns REX3’s plane writes into VRAM
words. Software never addresses RB2 directly; it is configured implicitly by REX3’s
`DRAWMODE` and `CONFIG`. [S11]

### 9.3 VC2 — video timing generator

VC2 holds the **video timing tables in external SRAM** and generates all sync signals,
blanking, and the pixel clock enable. Key facts: pixel clock **PCLK/2 ≤ 70 MHz**, maximum
pixel clock 140 MHz; it has “slow” (< 70 MHz) and “fast” (≥ 70 MHz) modes; the hardware
cursor is **32×32 at 2 bits/pixel** or **64×64 at 1 bit/pixel**; it supplies a
**Display ID (DID)** to the monitor via the sync signals. [S9]

Because the timing tables live in SRAM, **mode setting is not a register write but a table
download**: software must generate the full timing table (H total, H sync start/end, H
blank start/end, V total, V sync, equalisation and serration pulses for interlaced modes)
and DMA it into VC2’s SRAM before enabling the pixel clock. Indy’s PROM contains tables for
the standard modes; a bare-metal OS can either copy them from PROM or synthesise them from
a VESA-style modeline.

Supported Indy modes (product-level): 1280×1024 at 50/60/72/76 Hz, 1024×768 at 60/70 Hz,
plus the lower-resolution modes used by IRIX. The 13W3 connector carries **sync-on-green**
and separate H/V sync; the monitor type is selected by the DID bits, and getting them wrong
produces a usable picture only on multisync monitors.

### 9.4 XMAP9 — the RAMDAC / pixel formatter

XMAP9 converts the logical pixel from VRAM into the analogue RAMDAC input. It supports
**24/12/8/4-bit RGB** and **12/8/4-bit colour-indexed** modes, an **8-bit auxiliary**
(a.k.a. overlay) buffer, **32 display IDs**, and up to **70 MHz**. A board may carry **two
XMAP9s** (even and odd pixels) for higher pixel rates. [S10]

The palette (CLUT) lives behind the XMAP9 and is loaded through REX3’s DCB bridge, not by
direct CPU access. In 8-bit mode the CLUT is 256×24; in 24-bit mode the CLUT is bypassed.
The auxiliary buffer is what makes the hardware cursor and overlay planes possible without
touching the main framebuffer.

### 9.5 CMAP and BT445

The colour map is a **Brooktree BT445** (or compatible) 256×24 RAMDAC in the earliest
boards; later boards integrate the palette into XMAP9. The BT445 is programmed through four
write-only registers (address, palette data, overlay/read mask) reached via REX3’s DCB
bridge at base + 0x0100/0x0104. Its pixel clock limit is 110 MHz (135 MHz on the -135
grade), comfortably above Indy’s 140 MHz pixel clock only because the XMAP9 divides it.
[S15]

### 9.6 What a graphics driver must do

1. **Reset** the board via MC `cpuctrl0.GFXRESET` (bit 10), then release.
2. **Program REX3 `CONFIG`** for GIO32 vs GIO64 and the FIFO watermarks. Wrong values give
   a board that draws nothing and never interrupts.
3. **Download the VC2 timing table** for the chosen mode and start the pixel clock.
4. **Program XMAP9** for the pixel format (depth, RGB vs CI, aux buffer), and load the CLUT.
5. **Program REX3 `DRAWMODE0/1`** for depth and logical op, and the five span masks for
   the visible region.
6. Draw. Every primitive is a write into the command FIFO; `GO` is implicit on the first
   write after the pipeline is idle.
7. For a linear framebuffer, either use `HOSTRW` (slow, 8- or 32-bit) or render via REX3
   (fast) — there is no third path.

### 9.7 Hard limits

* **No Z-buffer and no geometry engine on XL/Newport.** 3D is software rasterisation.
* 1280×1024 maximum, 64 pixels of offscreen.
* 140 MHz pixel clock ceiling; XZ reaches 1280×1024 at 76 Hz with two XMAP9s.
* Colour is planar in VRAM; “24-bit” means three 8-bit planes, not packed pixels.
* The command FIFO is small; sustained fill rate is limited by REX3’s 33 MHz clock and the
  GIO32 path if the board is in a GIO32 slot.
* **VRAM size per variant is not documented in the sources available here.** 8-bit XL is
  commonly reported as 1 MB and 24-bit XL as 2 MB, but 1280×1024×24 needs 3.75 MB, so
  either the 24-bit board carries more than 2 MB or it cannot do 24-bit at 1280×1024.
  This is flagged in [§18](#18-unresolved-and-uncertain); a driver should size VRAM from
  the board ID / ARCS rather than assume.

### 9.8 XZ (Extreme)

Two boards: a “GE” board carrying **four geometry engines** derived from Elan, and a
framebuffer board with **Z-buffer** and 24-bit output. The RE3 thread owns the VRAM and
draws from its FIFO; the geometry engines execute display-list commands. XZ is
**not** register-compatible with XL — it has its own command stream, its own FIFO protocol,
and its own interrupt set. [S20 `docs_indy-xz-elan.md`]

---

## 10. Vino — video capture

**Vino is input-only.** It captures composite/S-Video (via the SAA7191B decoder) and
optionally digital video, and DMAs frames into memory. It cannot drive a display; output
requires the separate “Indy Video” GIO32 board (which also provides the video outputs and
genlock), and compression requires CosmoCompress. [S12 §1; S14]

### 10.1 Address map

Vino’s register space starts at **0x0008_0000 within the graphics slot window**, i.e.
**0x1F08_0000** with the Indy slot base of 0x1F00_0000. It is a **64-bit real-time GIO64
device**: a slave for register access, a master for DMA. It uses the `EISA_Req_n`,
`EISA_Gnt_n`, `EISA_AS_n` signals, which give it the **highest priority** of all GIO64
devices. [S12 §2, §2.4]

### 10.2 Registers

All accesses are **64-bit doubleword** accesses on doubleword boundaries, regardless of the
register width; for 32-bit access use the +4 address of a +0 register. Block transfers are
not supported. Unused bits read as zero, and the GIO byte count is ignored (eight bytes are
always transferred). [S12 §2.1, §2.2]

Global registers (not duplicated per channel):

| GIO address | Function | Access |
|---|---|---|
| 0x0008_0000 | Revision / ID | read |
| 0x0008_0008 | Control (both channels) | read/write |
| 0x0008_0010 | Interrupt status (both channels) | read/write |
| 0x0008_0018 | I²C control/status | read/write |
| 0x0008_0020 | I²C data | read/write |

Channel A registers; **channel B is the same layout at +0xB0**:

| GIO address (A / B) | Function |
|---|---|
| 0x0028 / 0x00B0 | Alpha |
| 0x0030 / 0x00B8 | Clipping start (X in 26:16, Y in 10:0) |
| 0x0038 / 0x00C0 | Clipping end (X in 26:16, Y in 10:0) |
| 0x0040 / 0x00C8 | Frame rate: bits 12:1 mask value, bit 0 mode (0 = NTSC, 1 = PAL) |
| 0x0048 / 0x00D0 | Field counter (read; reset by enabling DMA) |
| 0x0050 / 0x00D8 | Line size (bytes per line **minus 8**) |
| 0x0058 / 0x00E0 | Line count (current byte-in-line count, +8 per transfer) |
| 0x0060 / 0x00E8 | Page index (9 bits, doubleword index into the 4 KB page) |
| 0x0068 / 0x00F0 | Pointer to next four descriptors |
| 0x0070 / 0x00F8 | Pointer to start of descriptor table |
| 0x0078 / 0x0100 | Descriptor 0 data |
| 0x0080 / 0x0108 | Descriptor 1 data |
| 0x0088 / 0x0110 | Descriptor 2 data |
| 0x0090 / 0x0118 | Descriptor 3 data |
| 0x0098 / 0x0120 | FIFO threshold |
| 0x00A0 / 0x0128 | FIFO read pointer (GIO side, read only) |
| 0x00A8 / 0x0130 | FIFO write pointer (video side, read only) |

### 10.3 DMA descriptors

A descriptor is a **single 32-bit word**:

```
bit 31      STOP
bit 30      JUMP
bits 29:0   physical address of a 4 KB page (bits 11:0 must be zero)
            (validity is a separate 33rd bit in the descriptor cache, read-only)
```

* **STOP** halts DMA immediately and raises an interrupt. The channel must be re-initialised
  (page index to zero, rewrite the descriptor pointers) and the bit cleared before DMA can
  resume.
* **JUMP** means the pointer field is the **quadword address of the next descriptor** instead
  of a page buffer. Jumps can only target quadword addresses.
* The stop and jump bits are ORed into the address and driven low during the GIO address
  cycle.

Descriptors are packed into a table that must be **quadword (16-byte) aligned**; the hardware
fetches **four descriptors at a time** (16 bytes) and advances the next-descriptor pointer by
16 after each fetch. The last descriptor in a table **must** have STOP or JUMP set or the
engine runs away through memory. [S12 §2.3.1]

Because each descriptor covers exactly **one 4 KB page**, a capture buffer must either be
4 KB-page-aligned in physical memory or described by a descriptor per page. There is no
scatter/gather within a descriptor, and no byte-granular offset: the DMA address is
`descriptor[29:12] << 12 | page_index[11:3]`.

### 10.4 Capture pipeline

1. **SAA7191B** (I²C, driven through Vino’s I²C port) decodes composite/S-Video into 4:2:2
   YUV and generates HSYNC/VSYNC/field. Field 1 vs field 2 is signalled by the level of
   HREF at the de-assertion of VS. The I²C interface is **single-byte and polled**: writing
   the data register starts the transfer, and software must poll the control register for
   completion before the next byte. [S12 §2.7, §3.1]
2. **Channel A** takes the video, applies clipping (X/Y start and end), optional alpha, and
   writes YUV 4:2:2 into memory. **Channel B** can capture a second stream concurrently.
3. Two DMA modes: **field capture** (each field starts on a 4 KB boundary, gaps between
   fields are normal) and **frame assembly / interleaving** (line 0 of field 1, skip a line,
   line 2 … then fill the skipped lines from field 2). Frame assembly requires `line_size`
   and `line_count` to be programmed.
4. Each channel has a **1 KB FIFO (128 × 64 bits)** with a programmable threshold. Vino
   requests the GIO64 bus when the FIFO reaches the threshold or at end-of-field, and holds
   the bus until the FIFO is empty. There is no hardware limit on bus tenure.
5. **Initialisation order matters:** write `page_index = 0` **before** `next_descriptor`,
   otherwise the page-index rollover shifts the freshly fetched descriptors and the first
   field lands in the wrong place. Then enable the channel in the control register.

### 10.5 Pixel formats and colour conversion

Vino outputs **4:2:2 YUV** (U and V shared between adjacent Y samples). It can optionally
convert on the fly:

* **YUV → 24-bit RGB**: control register bit 11 (channel A) / bit 23 (channel B).
* **Dither to 8-bit RGB**: bit 18 (A) / bit 30 (B), applied after the 24-bit conversion.
* **Monochrome**: U and V discarded, only Y retained — the cheapest mode in bus bandwidth
  and memory.

The conversion is done with shifts and adds (no interpolation from 4:2:2 to 4:4:4; the UV
pair is simply repeated for the second Y), with an error below ±0.6 LSB and clamping to
0…255. Equations as specified: [S12 §4.5]

```
R = 1.596027·(V'−128) + 1.164384·(Y'−16)
B = 2.017232·(U'−128) + 1.164384·(Y'−16)
G = 1.164384·(Y'−16) − 0.8129676·(V'−128) − 0.3917623·(U'−128)
```

### 10.6 Interrupts

Six sources, all **level-sensitive** inside Vino: channel A end-of-descriptor-table, A FIFO
overflow, A end-of-field, and the same three for channel B. Clearing requires removing the
condition (for end-of-field, which has no condition, it can be cleared immediately; for FIFO
overflow and end-of-table, DMA must be disabled and re-initialised).

The GIO interrupt line is **edge-sensitive at the IOC**, which creates a documented
pathological case: if channel B raises an interrupt in the window between the handler
polling the status and clearing channel A, the physical line never de-asserts and **all
subsequent Vino interrupts are lost**. SGI’s recommended workaround is to disable the other
channel’s interrupt while clearing the one being serviced. [S12 §2.5] This is not optional
advice; it is a real deadlock.

**Reset.** Vino is reset by an IOC register bit (the video reset), held low for ~200 ms by
the Philips 7197 clock generator after release. The bit is cleared on power-up, so software
must set it explicitly before touching Vino. [S12 §2.6]

### 10.7 Hard limits

* **Input only.** No output, no display, no genlock without Indy Video.
* **Frame-rate mask is 12 bits.** Bit 0 of the frame rate register selects NTSC (0) or PAL
  (1); bits 12:1 mask which fields are captured. NTSC has 12 fields per frame-rate cycle and
  PAL 10, so on PAL only the low 10 mask bits are meaningful. A field turned off by the mask
  produces no end-of-field interrupt.
* **No arbitrary scaling.** The only reduction is field decimation through the frame-rate
  mask; there is no line or pixel decimation register in the map. Resolution reduction must
  be done by clipping plus software.
* Buffers are addressed **one 4 KB page per descriptor**, and the descriptor table must be
  16-byte aligned. There is no byte-offset field and no scatter/gather inside a descriptor.
* The FIFO is 1 KB; a line shorter than 8 bytes is illegal (`line_size` = bytes − 8, and
  lines must be a multiple of 8 bytes).
* Vino has **no compression** and its colour conversion is 4:2:2-based without chroma
  interpolation, so RGB output has visible chroma aliasing at hard colour edges.
* The GIO interrupt is edge-sensitive at the IOC while Vino’s sources are level-sensitive;
  the two-channel clearing protocol in §10.6 is mandatory.
* Vino has the **highest GIO64 priority** of any device and no bus-tenure timer, so a
  high-rate capture can measurably slow the CPU.

---

## 11. RTC and NVRAM — Dallas DS1386

**DS1386-8K** at **0x1FBE_0000**, in **byte lane 3** of each 32-bit word: register *n* is at
`(n << 2) + 3`. It provides a calendar (valid to 2100, no century register), a watchdog, an
alarm, and **8 KB of battery-backed SRAM** (32 KB on the -32K part). [S13; S18
`ip22-nvram.c`; S19 `dsclock.c`]

### 11.1 Register map

| Reg | Contents | Format |
|---|---|---|
| 0x00 | hundredths of a second | BCD |
| 0x01 | seconds | BCD |
| 0x02 | minutes | BCD |
| 0x03 | minutes alarm | BCD, bit 7 = mask |
| 0x04 | hours | BCD, bit 6 = 12/24 select, bit 5 = PM or 20-hour bit |
| 0x05 | hours alarm | BCD, bit 7 = mask |
| 0x06 | day of week | BCD, 1–7 |
| 0x07 | date alarm | BCD, bits 6:3 always 0, bit 7 = mask |
| 0x08 | date | BCD |
| 0x09 | month | BCD; bit 7 `EOSC` (0 = oscillator running), bit 6 `ESQW` (0 = 1024 Hz output, 1 = high-Z) |
| 0x0A | year | BCD, 00–99 |
| 0x0B | command | binary (see below) |
| 0x0C/0x0D | watchdog alarm LSB/MSB | BCD, 0.01 s units |
| 0x0E–0x1FFF | user RAM (8K part) | raw |

**Command register (0x0B)** — binary, not BCD:

| Bit | Name | Meaning |
|---|---|---|
| 7 | `TE` | transfer enable — **0 freezes the time registers** |
| 6 | `IPSW` | interrupt switch: 1 → INTA = time-of-day alarm, INTB = watchdog; 0 → reversed |
| 5 | `IBH/LO` | INTB sink/source current select |
| 4 | `PU/LVL` | 1 = pulse mode (≥3 ms), 0 = level mode |
| 3 | `WAM` | watchdog alarm mask (0 = enabled) |
| 2 | `TDM` | time-of-day alarm mask (0 = enabled) |
| 1 | `WAF` | watchdog alarm flag, **read-only**, cleared by accessing the watchdog registers |
| 0 | `TDF` | time-of-day flag, **read-only** |

**Reading time safely.** Either (a) clear `TE` (write 0x00 to 0x0B), read registers
0x00–0x0A, then set `TE` again; or (b) read twice and compare. The first method is what
NetBSD does. Note the century: the DS1386 has **no century register**, so software must
apply the epoch convention (SGI systems use 1900 + year for years ≥ 70 and 2000 + year
otherwise, matching the PROM’s Y2K fix).

### 11.2 NVRAM layout

The 8 KB is shared between the PROM environment and the MAC address:

| Offset | Contents |
|---|---|
| 0x000–0x3FF | PROM environment variables (see §13.3) |
| 0x4E8 | Ethernet MAC address, 6 bytes |
| 0x4EE–0x7FF | PROM scratch / diagnostic results |
| 0x800–0x1FFF | free for the OS |

A dead lithium cell makes the whole array read `0xFF`, including the MAC — hence the
`ff:ff:ff:ff:ff:ff` case in §6.5. When the checksum of the environment block is invalid,
ARCS restores defaults.

### 11.3 Watchdog

The DS1386 watchdog compares the time registers against the watchdog alarm registers
(0x0C/0x0D) and asserts INTA/INTB or, if strapped that way, drives the system reset. `WAM`
(bit 3 of the command register, 0 = enabled), `PU/LVL` (bit 4) and `IPSW` (bit 6) select the
behaviour; `WAF` (bit 1) reports the alarm. **No open-source OS on Indy uses it** — Linux and
NetBSD both leave it masked. It is a plausible last-resort reset mechanism for a bare-metal
kernel, but note that the alarm is expressed in BCD hundredths of a second, so the maximum
interval is 99.99 s and the resolution 10 ms.

### 11.4 Hard limits

* Register access is **byte-wide, lane 3 only**; a 32-bit read returns the byte in bits
  31:24 and garbage in the rest.
* Only a **two-digit year** is stored; mapping it to a century is the software’s job (the
  PROM applies its own Y2K window).
* Timekeeping accuracy is that of a cheap 32.768 kHz crystal (±1–2 min/month); there is no
  temperature compensation.
* The battery is soldered inside the DS1386 module — when it dies the whole chip (RTC +
  NVRAM + MAC) must be replaced, which is why so many Indys have lost their MAC address.

---

## 12. GIO expansion bus

GIO is Indy’s graphics/expansion bus: **GIO64** for the graphics slot, **GIO32** for the
two expansion slots. It is a multiplexed address/data bus, synchronous to a 25/30/33 MHz
(GIO32) or up to 40 MHz (GIO64) clock, with central arbitration in the MC. [S4]

### 12.1 Electrical and protocol summary

* **GIO64**: 64-bit multiplexed AD, separate address phase, up to **40 MHz**, peak
  **320 MB/s**; pipelined and non-pipelined transfers; real-time devices get priority.
* **GIO32**: 32-bit, 25–33 MHz, **44 MB/s** writes and about **16 MB/s** reads from memory
  (theoretical 26 MB/s). Measured Indy PIO: **13 MB/s read, 66 MB/s write** to a GIO device.
* Signals: `AD<63:0>`, `ADD<31:0>` in the address phase, `MAS<3:0>` (byte enables),
  `BREQn`/`BGNTn` arbitration, `MASDLY`/`SLVDLY` handshake, `RD/WR`, `DS`, `ACK`, `ERR`,
  `RT` (real-time), `ENDIAN`.
* **Byte numbering is big-endian**, and `MAS<n>` corresponds to byte *n* in big-endian
  order. There is a per-slot **endian bit** in the MC’s `giopar` register, plus fixed-endian
  override bits in `cpuctrl1` for HPC, EXP0, and EXP1 — these exist because some third-party
  GIO boards are little-endian.

### 12.2 Arbitration and the DMA TLB

GIO masters request the bus with `BREQn`; the MC grants with `BGNTn`. A master **must keep
accepting requests even while requesting**, to avoid deadlock. Real-time devices (Vino, and
any slot marked real-time in `giopar`) are serviced first, in the order: Vino, GFX, EXP0,
EXP1.

The **VDMA** engine in the MC performs DMA for masters that cannot generate addresses
themselves (notably the HPC3’s PBUS channels and the parallel port). It has:

* a **4-entry micro-TLB** mapping `PTEBase` entries,
* a **4-entry DMA TLB** (`dtlb_hi0..3`/`dtlb_lo0..3`),
* a **single descriptor per transfer** with an unlimited contiguous size,
* GIO `MASK`/`SUBST` address protection,
* registers `MEMADR`, `MEMADRD`, `SIZE`, `STRIDE`, `ADR`, `ADRS`, `MODE`, `COUNT`,
* a default descriptor of line count 1, stride 0, zoom 1, line width 12, byte count 12.

Modes: read, write, and accumulate. Options: fill, sync (`DMASYNC` / vertical retrace),
long burst, snoop. [S5]

**Practical consequence for a kernel:** if you write a GIO driver, you must either (a) use
PIO only, or (b) program the MC’s DMA TLB so the device’s 32-bit “address” translates to a
real physical page, because GIO devices see only 32-bit addresses and the MC’s translation
is the only remapping layer.

### 12.3 Slots and identification

| Slot | Base | Type |
|---|---|---|
| Graphics | 0x1F00_0000 (device at 0x1F0F_0000) | GIO64 |
| EXP0 | 0x1F40_0000 | GIO32 |
| EXP1 | 0x1F60_0000 | GIO32 |

Each slot responds to a read of its base address with a **Product Identification Word**:

* bits 7:0 — **Product ID code** (unique, assigned by SGI; bit 7 = 0 means the rest of the
  word is invalid),
* bits 15:8 — manufacturer code,
* bits 23:16 — revision,
* bit 31 — valid.

A driver should read this word first and refuse to touch a slot whose ID it does not
recognise. Indy’s own IDs: graphics boards, Vino, and the Indy Video board each have a
distinct code; **the full SGI product-ID table is not in the documents available here**
(flagged in §18).

GIO interrupts arrive as `SGI_GIO_0_IRQ` (local0 bit 0), `SGI_GIO_1_IRQ` (local0 bit 6),
`SGI_GIO_2_IRQ` (local1 bit 7), and `SGI_GIOEXP0_IRQ`/`SGI_GIOEXP1_IRQ` (local2 bits 6/7).

### 12.4 Hard limits

* GIO32 slots are **not** hot-pluggable and have no configuration space beyond the product
  word — no BARs, no interrupt pin registers. Address decode is fixed by the slot.
* **32-bit addressing only.** A GIO device cannot see memory above 4 GB (moot on Indy) and
  cannot see the upper 256 MB of a 512 MB machine without translation.
* PIO reads are **slow** (13 MB/s measured); never use PIO for bulk data.
* The bus has **no cache coherency**. Every DMA buffer needs explicit cache maintenance.
* Real-time devices (Vino) can hold the bus for a long time; a driver must not spin on a
  GIO register with interrupts disabled for long.

---

## 13. PROM and ARCS

### 13.1 Layout

The PROM is **32-bit ARCS** firmware (not 64-bit ARCS64, which is Octane/Origin). It lives
at **0x1FC0_0000**; the range **0x1FE0_0000–0x1FFF_FFFF is Flash**, separately mapped and
not an alias of the PROM. [S3 §6.0] The ARCS **parameter block** is at **0xA000_1000**
(a fixed KSEG1 address, i.e. physical 0x2000_1000). [S18 `sgiarcs.h`; S20]

```c
struct _SYSTEM_PARAMETER_BLOCK {
    u32 magic;        /* 0x53435241 == "SCRA" */
    u32 len;
    u16 ver, rev;
    u32 rs_block, dbg_block, gevect, utlbvect;
    u32 rveclen;  void *romvec;
    u32 pveclen;  void *pvector;
    u32 adap_cnt, adap_typ0, adap_vcnt0;  void *adap_vector;
    u32 adap_typ1, adap_vcnt1;            void *adap_vector1;
    ...
};
```

`romvec` points to a table of function pointers, in this order: `load`, `invoke`, `exec`,
`halt`, `pdown`, `restart`, `reboot`, `imode`, `_unused1`, then the device-tree calls
(`next_component`, `child_component`, `parent_component`, `component_data`, `child_add`,
`comp_del`, `component_by_path`), `cfg_save`, `get_sysid`, `get_mdesc`, `_unused2`,
`get_tinfo`, `get_rtime`, then file ops (`get_vdirent`, `open`, `close`, `read`,
`get_rstatus`, `write`, `seek`, `mount`), then `get_evar`/`set_evar`, `get_finfo`/`set_finfo`,
`cache_flush`, `TestUnicodeCharacter`, `GetDisplayStatus`.

**All ARCS calls are made through the firmware’s own calling convention**, which is the
standard 32-bit MIPS ABI (`a0`–`a3` arguments, `v0` return, `ra` return address), and the
firmware runs with its own stack and its own exception handlers. A kernel that calls ARCS
after taking over the TLB and caches must be extremely careful: ARCS expects the memory map
it installed at boot. The usual pattern (Linux, NetBSD) is to call ARCS **only during early
setup**, before replacing the TLB, and to copy anything needed (memory descriptor list,
environment, device tree) into kernel memory first.

### 13.2 Memory descriptor list

`get_mdesc` returns a linked list of `{u32 base, u32 pages}` describing usable RAM. On Indy
this is normally one or two entries covering 0x0800_0000 upward. Do not assume contiguity
across the 0x1800_0000 boundary; do not assume the list is sorted.

### 13.3 Environment variables

Stored in DS1386 SRAM at offset 0, ASCII `name=value` NUL-separated, terminated by an empty
name, with a checksum. Names a kernel may care about: `console` (`dksc`, `serial0`,
`serial1`, `gfx`), `eaddr` (MAC override), `dbaud`/`rbaud` (debug/remote console baud),
`netaddr`, `netmask`, `syspart`, `osloader`, `bootfile`, `volume`, `keybd`, `nonstd`
(the last selects non-standard monitor timings). The set is firmware-defined and **the
complete list is not published in the sources consulted** — the practical approach is to
enumerate it from the block at boot and print it.

### 13.4 PROM versions and boot path

Versions 004 through 011 shipped; **011 is required for R5000** machines (earlier PROMs do
not initialise the R5000’s L2 and hang). The default boot path is
`dksc(0,6,8)sashARCS` and the standalone diagnostics are `stand/fx.ARCS`. The PROM’s
`hinv` command is the authoritative inventory of what the firmware believes is installed —
useful for cross-checking a kernel’s own probing.

### 13.5 Hard limits

* **32-bit only.** No 64-bit ARCS calls, no device tree with 64-bit addresses.
* The PROM occupies the low 512 KB of the physical map; a kernel must not overwrite
  0x1FC0_0000–0x1FC7_FFFF if it wants to warm-reboot through the PROM.
* ARCS is **not re-entrant** and is not safe to call with interrupts enabled once the
  kernel owns the interrupt controller.
* The `imode` entry point drops to the PROM’s command monitor; on Indy it requires a
  working serial console or graphics console, and it will not return.

---

## 14. Interrupts, timers and timebase

### 14.1 The three-level interrupt tree

```
CPU IP2 ── HPC3 (istat0/istat1, 10 sources)
CPU IP3 ── local1 (panel, HPC DMA done, AC fail, Vino, GIO2)
CPU IP4 ── 8254 timer 0        (broken, see below)
CPU IP5 ── 8254 timer 1        (broken)
CPU IP6 ── bus error (MC parity)
CPU IP7 ── CP0 Count/Compare   ← the only usable periodic tick
```

`local2` is the IOC2’s **INT3** vectored interrupt multiplexer, at **0x1FBD_9880**
(`INT2_IP24`). It carries EISA (Indigo2), keyboard, serial, GIO EXP0, GIO EXP1, and the
graphics vertical-retrace interrupt. `local0` bit 0 is the “FIFO full” / GIO-0 line,
bit 6 is GIO-1 / second HPC.

Full mapping (from [S18 ip22.h]):

| Line | Source |
|---|---|
| IP2 | HPC3 cascade (PBUS 0–7, SCSI 0, SCSI 1) |
| IP3 | local1: GPL0, **panel**, GPL2, MAP1, **HPC DMA done**, **AC fail**, **Vino**, GIO2 (vert retrace) |
| IP4/IP5 | 8254 counters 0/1 (unusable) |
| IP6 | bus error |
| IP7 | CP0 timer |
| INT3.0 | graphics vertical status |
| INT3.3 | EISA |
| INT3.4 | keyboard |
| INT3.5 | serial |
| INT3.6 | GIO EXP0 |
| INT3.7 | GIO EXP1 |

### 14.2 The 8254 bug

The IOC2 contains an Intel 8254-compatible timer clocked at **1 MHz**, but its **interrupt
output is not connected correctly** on Indy — SGI documents it as a bug, and both Linux and
NetBSD use it only as a **counter**, never as an interrupt source. [S18 `ip22-time.c`
comment: “as an interrupt driving timer it does not because of bug”]

This is genuinely useful: the 8254 counters still count, so a kernel can **calibrate CP0
Count against a known 1 MHz reference** at boot:

1. Program counter 2 in mode 0 (interrupt on terminal count) with a large reload value.
2. Read CP0 `Count`, then poll the 8254’s latched counter until its top byte reaches zero.
3. Read CP0 `Count` again; the delta is the number of CP0 ticks per (reload / 1 MHz)
   seconds. Round to the nearest MHz.
4. Compute `r4k_offset = delta / HZ` and use it to reload `Compare` on every tick.

Linux does exactly this in `dosample()` [S18 `ip22-time.c`]. A Rust kernel should do the
same rather than assuming the core clock, because Indy shipped at 50, 75, 100, 150, and
180 MHz and the PROM does not always report the right value.

### 14.3 Timebase

* CP0 `Count` increments **every second core clock** (R4000/R4400) or at half the core
  clock (R4600/R5000). At 100 MHz that is a 20 ns or 10 ns tick — 32 bits wrap in 20–40 s,
  so a 64-bit software timebase must extend it in the timer interrupt.
* Writing `Compare` **clears** `Cause.IP7`; there is no separate ack. If the new value is
  already in the past the interrupt re-fires immediately — always write `Count + offset`.
* The **RPSS** counter in the MC is a free-running 100 ns counter used by IRIX profiling;
  it is a convenient monotonic clock that does not wrap as fast as `Count`.
* The DS1386 provides wall-clock time at 10 ms resolution; it is the only persistent
  timebase.

### 14.4 Programming order for a tick

1. Calibrate as above; store `cycles_per_tick`.
2. `write_c0_compare(read_c0_count() + cycles_per_tick)`.
3. Unmask IP7 in `Status.IM` and set `Status.IE`.
4. In the handler: read `Cause`, if IP7 then `write_c0_compare(read_c0_count() + cycles_per_tick)`
   (this acks), then do the tick work.

**Hard limits.** The timer has no prescaler and no one-shot mode; it is a free-running
comparator. Resolution equals the core clock period, which is fine, but the 32-bit width
means a tick longer than ~40 s at 100 MHz is impossible without a software extension.

---

## 15. Miscellaneous: power, LEDs, front panel, serial console

### 15.1 Power control (soft power)

The front-panel button does **not** switch the supply directly. It signals the IOC2, which
drives the supply’s `PS_ON` line. The IOC2’s `panel` register (0x1FBD_9850) reports:

| Bit | Meaning |
|---|---|
| 0 | `POWERON` — supply is on |
| 1 | `POWERINTR` — button was pressed |
| 4 | `VOLDNINTR` — volume-down button edge |
| 5 | `VOLDNHOLD` — volume-down held |
| 6 | `VOLUPINTR` — volume-up button edge |
| 7 | `VOLUPHOLD` — volume-up held |

To power off: clear `POWERON` (the IOC2 then drops `PS_ON`). To respond to the button:
enable the panel interrupt (`SGI_PANEL_IRQ`, local1 bit 1), read `panel`, and act on
`POWERINTR`. **There is no software-readable absolute volume** — only up/down edges and
hold, so a kernel must integrate them into its own volume variable and drive the HAL2
attenuation accordingly.

### 15.2 LEDs

Two bicolour LEDs are driven by `reset` bits 5:4 (`SGIOC_RESET_LC0OFF`, `LC1OFF`). The
encoding is deliberately awkward: on Indy, clearing LC0 turns the **red** LED *off* and
leaving it set turns it **green** (or amber, depending on the other bit). NetBSD’s
`button.c`/`imc.c` and Linux’s `ip22-reset.c` both treat the pair as a 2-bit state machine.
A kernel that wants a heartbeat LED should drive both bits through a small state table
rather than assuming “1 = on”.

### 15.3 Serial console — Z85C30 inside IOC2

The IOC2 embeds a **Zilog Z85C30 ESCC** with SGI’s “85CX30” enhancements (4-byte TX FIFO,
8-byte RX FIFO, write register 7′, latched RR0). Base **0x1FBD_9830**, indirect register
access: write the register number to `(reg << 2) + 3` and then the data to the same
address (the ESCC’s classic two-step access, with the byte in lane 3).

**The ports are wired in reverse order**: IOC2 port 1 is ESCC **channel B**, port 2 is
**channel A**. ARCS addresses them as `serial0` and `serial1`; `serial0` is the one on the
rear panel labelled “Serial Port 1”. Baud rate is derived from a **3.672 MHz** PCLK, giving
the standard rates; **9600 8N1** is the PROM default and the safe bring-up choice.

Each channel can be **RS-232 (ARC) or RS-422 (Mac)**; the mode is selected per channel by
`write` register bits 4 and 5 (`U1AMODE`, `U0AMODE`: 1 = PC/RS-232, 0 = Mac/RS-422). The
Mac mode is the reset default, which is why a fresh Indy often needs a Mac-style mini-DIN
cable or an explicit mode switch. There is no hardware flow control wiring on the Indy
connector; RTS/CTS are looped or absent depending on the cable.

**Console selection** is by the PROM `console` variable. If it is `serial0`/`serial1`, the
PROM does all its I/O on that ESCC channel and the graphics board is never initialised —
which is convenient for a headless bring-up but means you cannot assume the framebuffer is
programmed when you take over.

### 15.4 Parallel port

The IOC2’s PI1 parallel interface (base 0x1FBD_9800) supports SPP, SGI parallel, HP-Boise,
and a Ricoh scanner mode, with a DMA channel selected by `dmasel` bit 2. For a bare-metal
OS it is the lowest-priority block: SPP mode is a handful of registers and an interrupt
(`SGI_PARPORT_IRQ`, local0 bit 5), and nothing on an Indy requires it.

### 15.5 Keyboard and mouse

An **82C42**-compatible controller inside the IOC2 at 0x1FBD_9840/0x1FBD_9844, with the
standard `STATUS`/`DATA` pair in lane 3. Interrupt is INT3.4. The controller also drives
the **PS/2-style** keyboard/mouse connectors; the mouse is a mechanical or optical
three-button unit and the protocol is ordinary PS/2 with the usual `0xE0` extended codes
for the middle button.

---

## 16. Suggested bring-up order for a bare-metal kernel

1. **Serial first.** Program the Z85C30 for 9600 8N1 on channel A, polled output only.
   Everything else is debuggable only if you can print.
2. **Exception vectors and CP0.** Install handlers at 0x8000_0000 (BEV = 0) and keep a
   BEV = 1 fallback that prints through the PROM.
3. **Memory size from ARCS `get_mdesc`**, before touching the TLB. Build a static direct
   map with 16 MB pages for RAM and 4 KB pages for I/O.
4. **CP0 timer.** Calibrate against the 8254 counter (§14.2) and enable IP7. Do not enable
   any other interrupt yet.
5. **HPC3 interrupt demux.** Read both `istat0` and `istat1` and synthesise the 10-bit
   status; install a spurious-interrupt counter to catch mistakes early.
6. **RTC.** Read the DS1386 with `TE` cleared; print the date. This validates byte-lane-3
   access, which is the same pattern used by the ESCC, the 82C42, and the WD33C93.
7. **Ethernet** in loopback (`reset.LBACK`), then on the wire. It is the least destructive
   block to debug and exercises the HPC3 descriptor engine.
8. **SCSI.** Start with a single 512-byte read, then build a chain. Expect the EOX
   terminator and the 8 KB cap to be the two things that go wrong.
9. **Graphics.** Only after everything else: mode setting requires the VC2 table download
   and is the most likely thing to leave the machine in a state where you cannot see
   output.
10. **Audio, Vino, ISDN** last. Audio is straightforward once PBUS DMA works; Vino needs
    the SAA7191B I²C dance; ISDN is undocumented.

Cache maintenance must be inserted at steps 7–9 for every DMA buffer, in both directions.

---

## 17. Sources

| Tag | Source | Location |
|---|---|---|
| S1 | SGI, *HPC3 Controller* chip specification (65 pp., SGI CONFIDENTIAL) | `hpc3.pdf` / `hpc3.txt` (local); §1.1, §3.0 address map, §2.6 peripheral ports, §5.0 misfeatures |
| S2 | SGI, *Memory Controller (MC)* chip specification (61 pp.) | `mc.pdf` / `mc.txt`; SIMM rules, `mconfig`, parity, bandwidth |
| S3 | SGI, *IOC2 Chip* specification (22 pp., 92/11/16 schematics) | `ioc.pdf` / `ioc.txt`; §2.6.3 ISDN glue, §4.0 register map, §6.0 Guinness/FullHouse differences |
| S4 | SGI, *GIO64 Bus Specification* (70 pp.) | `gio64.pdf` / `gio64.txt`; §2.10 product ID word, arbitration, bandwidth |
| S5 | SGI, *VDMA* specification (23 pp.) | `vdma.pdf` / `vdma.txt`; µTLB, DMA TLB, descriptor defaults, protection |
| S6 | MIPS Technologies, *R4000 Microprocessor User’s Manual*, 2nd ed. | `iris/R4000_um2.pdf` / `.txt`; ch. 4 caches, ch. 5 TLB, ch. 6 exceptions |
| S7 | IDT, *R4600 ORION Microprocessor User’s Manual* | `iris/r4600.pdf` / `.txt`; caches, TLB, `Config` |
| S8 | SGI, *REX3* graphics specification | `iris/rex3.pdf` / `.txt`; register table at `0x1FnF0000`, GO bit, pipeline |
| S9 | SGI, *VC2* video timing generator specification | `iris/vc2.pdf` / `.txt`; SRAM timing tables, PCLK limits, cursor, DID |
| S10 | SGI, *XMAP9* pixel formatter specification | `iris/xmap9.pdf` / `.txt`; 24/12/8/4-bit modes, 32 display IDs, 70 MHz |
| S11 | SGI, *RB2* VRAM formatter specification | `iris/rb2.pdf` / `.txt`; write mask, 66 MHz, 33 MHz sysclk |
| S12 | SGI, *Vino* video capture specification | `iris/vino.pdf` / `.txt`; §2.1 address map (`0x00080000`), Table 1 registers, §2.3 FIFO/DMA, descriptors |
| S13 | Dallas Semiconductor, *DS1386 RAMified Watchdog Timekeeper* datasheet, 041697 | `iris/Dallas_1386_Indy.pdf` / `.txt`; Figure 2 register map, command register, RAM layout |
| S14 | Philips, *SAA7191B* digital video decoder datasheet | `iris/SAA7191B.pdf` / `.txt`; I²C address, 4:2:2 output, sync generation |
| S15 | Brooktree, *BT445* RAMDAC datasheet | `iris/bt445_ramdac.pdf` / `.txt`; palette registers, pixel clock grades |
| S16 | Seeq Technology, *DQ8003* Ethernet controller datasheet | `iris/DQ8003_SeeqTechnology.pdf` — **scanned image, no extractable text**; register behaviour taken from S1, S18, S19 |
| S17 | Western Digital *WD33C93A/B* SCSI controller documentation | `iris/docs_Indy_wd33c93a.md` (distilled register reference) |
| S18 | Linux kernel, `arch/mips/sgi-ip22/` and `arch/mips/include/asm/sgi/` | `https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/arch/mips/include/asm/sgi/hpc3.h` (and `mc.h`, `ioc.h`, `ip22.h`, `gio.h`, `sgiarcs.h`); drivers `sgiwd93.c`, `sgiseeq.c`, `hal2.c`, `ip22-nvram.c`, `ip22-time.c`, `ip22-int.c`, `ip22-mc.c`, `ip22-reset.c` |
| S19 | NetBSD, `sys/arch/sgimips/` | `https://github.com/NetBSD/src/tree/trunk/sys/arch/sgimips`; `hpc/hpcreg.h` (PBUS/SCSI/ENET offsets, “HPCBUG” comment), `hpc/hpc.c`, `hpc/hpcdma.c`, `hpc/int.c`, `hpc/int2reg.h` (`INT2_IP24 0x1fbd9880`), `dev/dsclock.c`, `dev/dpclock.c`, `dev/scn.c`, `dev/imc.c`, `dev/wdsc.c`, `dev/if_sq.c`, `dev/haltwo.c`, `dev/haud.c`, `dev/button.c` |
| S20 | iris emulator documentation (hardware notes distilled from SGI specs) | `iris/docs_Indy_rex3.md`, `docs_Indy_hal2.md`, `docs_Indy_wd33c93a.md`, `docs_indy-xz-elan.md`, `docs_indigo2-ip22.md`, `docs_interrupt_map.md` |
| S21 | LinuxMIPS wiki and SGI product documentation (R5000, Indy modes, PROM versions) | `https://www.linux-mips.org/wiki/SGI_IP22`, `r5000.wiki` |

**Note on the SGI PDFs.** S1–S5 are SGI’s internal chip specifications; their PDF titles
point at SGI’s `specifications/chips` source tree. They circulate via the SGI hardware
documentation mirrors (`hardware.mc.pp.se`, the Linux-MIPS documentation archive, and the
iris emulator’s doc set). The local copies under `/tmp/opencode/` are authoritative for the
page and section numbers cited here; the mirror URL is not stable enough to cite per
document.

---

## 18. Unresolved and uncertain

These are the places where the sources conflict, are silent, or where I could not verify a
claim to primary-source standard. They are listed rather than smoothed over.

1. **SCSI DMA direction bit.** HPC3 spec and NetBSD say bit 2 = 1 means memory → device;
   Linux `hpc3.h` comments say the opposite. Recommended reading is 1 = mem→dev (§5.3),
   but this is not settled by a direct experiment in the sources consulted.
2. **ISDN.** Only the IOC2 glue is documented. The HSCX register window, the exact HSCX
   part (PEB 2085 vs 2086), and whether any Indy ever shipped with a working ISDN board are
   unverified. No open-source driver exists. Treat as reverse-engineering territory.
3. **XL/Newport VRAM sizes.** 1280×1024 at 24 bits needs 3.75 MB, but common product
   descriptions say 2 MB for the 24-bit XL. Either the depth/resolution combination is
   restricted, or the VRAM figure is wrong. A driver must not hard-code a size.
4. **Vino -001 errata.** The `JUMP`-to-self behaviour described in §10.7 is from emulator
   notes, not from the Vino specification’s errata section (which the local copy lacks).
5. **GIO product ID table.** The encoding is documented (S4 §2.10) but the assigned codes
   for Indy’s own boards are not in the available documents. Drivers should match on the
   full word or on a known board name from ARCS, not on a guessed ID.
6. **PROM environment variable list.** The names in §13.3 are the ones used by Linux/ARCS
   code paths; the complete set is firmware-defined and not published in the sources here.
7. **PBUS channel assignment on Indy.** The HPC3 has eight PBUS DMA channels; which channel
   is wired to HAL2, the parallel port, and ISDN on the Indy motherboard is not stated in
   the IOC2 or HPC3 specs. Linux’s HAL2 driver and NetBSD’s `haud.c` imply channels 0–3 for
   audio, but the mapping should be confirmed against a schematic.
8. **HPC3 `istat` bit 8/9 semantics.** Both OSes treat bit 8 as SCSI channel 0 and bit 9 as
   channel 1, and the spec agrees, but the Ethernet completion interrupt’s exact status bit
   (PBUS bit vs a dedicated bit) differs between the two drivers’ comments.
9. **8254 interrupt wiring.** Documented as “broken” by both OSes, but the precise failure
   mode (no IRQ line vs wrong polarity vs shared with the panel) is not stated anywhere in
   the sources; it is treated here as “do not use as an interrupt source”.
10. **XZ register-level programming.** The Elan-derived command stream is not documented in
    any of the available specifications; only the product-level description (four geometry
    engines, Z-buffer) is solid.
11. **DS1386 century handling and the exact PROM epoch split** (70 vs 69) is inferred from
    NetBSD’s `dsclock.c` and IRIX convention, not from an SGI document.
12. **Measured bandwidth figures** (13/66 MB/s GIO PIO, 30 MB/s HPC3 aggregate, 266 MB/s
    MC peak) are from SGI’s own MC/GIO documents and from driver comments; they were not
    re-measured here.
