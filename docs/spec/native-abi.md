# Native IRIX 7 capability ABI — cells, capabilities and invocation

**Status:** draft
**Version:** 0.1
**Owner:** kernel / ABI
**Layout source:** [`crates/native-abi`](../../crates/native-abi/README.md) — the
cell-side types land in M9; this document is normative for the kernel ABI until then.

**Related:** [`architecture.md`](architecture.md) §2 (D1–D32), §4, §5, §6 and §17.4;
[`native-libc.md`](native-libc.md) (the C ABI above this contract);
[`posix.md`](posix.md) (the source API); [`elf.md`](elf.md) (the IRIX compatibility
loader, not the native loader); [`driver-interface.md`](driver-interface.md)
(`ctl`/`data`/`status` and `posix_devctl`); [`modules.md`](modules.md) (the separate
kernel-module ABI); [`../plan/interfaces.md`](../plan/interfaces.md) (interface register);
[`../plan/milestones.md`](../plan/milestones.md) M9 and M9.5;
[`../research/rust-std-irix7.md`](../research/rust-std-irix7.md) §3, §5.4, §7.2–§7.5 and
§9.3; [`../research/rust-std-build-spike.md`](../research/rust-std-build-spike.md);
ADR-0011 (native capability substrate) and ADR-0021 (native Rust `std`).

> **This is a contract draft.**  It is **not** a frozen ABI, **not** a claim that the
> native capability ABI works, and **not** a substitute for the M9 conformance tests.
> No native cell loader, production libc or `std` PAL exists yet.  A static `std` image
> does run through the provisional syscall ABI and a 42-symbol shim, but that is a
> bring-up path, not this contract; it uses a small kernel fd table, while this ABI
> keeps the native fd table in libc.  Every numeric value, size and offset is provisional
> until the `native-abi` layout assertions, the C oracle and the M9 tests land.  Open
> items are listed in §10; do not implement one as if it were settled.

---

## 1. Scope and status

### 1.1 What this document defines

This is the kernel-side contract for **native cells**: the execution model, the
`CellStart` boot structure, the handle and rights model, the object and invocation
surface, the trap gate, cell lifecycle, threads, TLS, signals, the namespace and
file-descriptor model, versioning, limits and conformance.

It is the layer between the framekernel and the native libc.  The C ABI that a native
program sees — `struct stat`, `errno`, symbols, startup, the POSIX profile — is
[`native-libc.md`](native-libc.md).  This document owns the **kernel objects and the
invocation encoding** that the libc consumes.

### 1.2 What it is not

| Not this | Where it belongs |
|---|---|
| The IRIX 6.5 syscall ABI, number space, `errno`, `struct stat`, `sproc`, `syssgi` | [`elf.md`](elf.md) and the compatibility container (architecture §6) |
| The native libc C ABI, POSIX profile, `errno` values and public structs | [`native-libc.md`](native-libc.md) |
| The kernel-module service table and flat image ABI | [`modules.md`](modules.md) |
| A memory-isolation guarantee | This document states the SAS limits in §2.3; hardware isolation is future work |
| A frozen ABI or a working implementation | §10 and the status note above |

### 1.3 Decided, proposed and open

**Decided by the architecture and ADRs.**  The native ABI is new, n32, big-endian and
capability-backed (D2, D4, D11); native cells share the kernel address space under the
framekernel model for M9 (D8, D9, D17); authority is a capability, not a name; rights
attenuate and revocation is recursive; a cell has no ambient root; the native ABI is
distinct from IRIX 6.5.

**Proposed for review in this draft.**  The `CellStart` layout (§3); the 64-bit handle
packing and the selector encoding (§4); the native status codes (§4.5); the method
tables (§4.6); the file-descriptor table living in the libc and the open file
description living in the kernel `File` object (§7.4); the signal/event model (§6.4);
`$28` as the thread pointer (§6.3); and the conclusion that general `fork` cannot be
implemented in the v0.1 kseg0 SAS model (§6.5).

**Open.**  The per-cell address space versus SAS decision; user mode versus kernel-mode
cells; the cell image format and relocation subset; the signal delivery model; ELF TLS;
`fork`; quotas; the `errno` mapping; `abi_stable` at cell-to-cell seams; and the rest of
§10.

### 1.4 Terms

| Term | Meaning |
|---|---|
| **Cell** | A loaded Rust image with a capability table, a namespace and one or more threads.  A native process is one cell with a Unix personality (D3). |
| **Job** | The kernel object that owns a cell's resources and is the revocation root.  A cell has exactly one self `Job`; child cells have their own `Job`s. |
| **Thread** | A scheduled task inside a cell.  Threads share the cell's address space, capability table, namespace and file-descriptor table. |
| **Capability** | An unforgeable reference to an object, held in a per-cell table, with a rights bitmap.  The handle is the cell's name for it; the table entry is the authority. |
| **Handle** | A 64-bit value `(generation << 32) \| index` that names a capability table slot.  The null handle is zero.  Rights are **not** carried in the handle. |
| **Object** | A kernel entity with a type and a method table: `Job`, `Channel`, `Port`, `Vmar`, `Vmo`, `File`, `Socket`, `Device`, `Mmio`, `Irq`, `DmaWindow`, `Namespace`, `Thread`. |
| **Rights** | The 10-bit authority mask in a capability table entry: read, write, execute, map, DMA, invoke, transfer, duplicate, revoke, manage. |
| **Namespace** | A capability-held mount/bind tree with a root `File`.  A path is resolved inside a namespace; a path never grants authority by itself. |
| **Invocation** | A typed call from a cell to a kernel object through the native gate.  It is not an IRIX syscall. |
| **Gate** | The `syscall 0x1F7` instruction and its register convention (§5). |
| **Event** | A 32-byte record delivered through a `Port`: signal, child exit, readiness, IRQ, channel, timer. |
| **VMAR / VMO** | A virtual memory region and the memory object that backs it.  In the M9 SAS model the virtual address is the kseg0 direct map of the VMO's frames. |
| **FD** | A POSIX file descriptor: a libc table index that refers to a handle plus flags.  The kernel has no native fd table. |

### 1.5 Relationship to the compatibility container

The IRIX 6.5 container and the native world share **kernel objects**, not layouts.

- A legacy process is a user-mode MIPS context in its own kuseg MMU address space with
  an ASID.  It enters the kernel on a plain `syscall` and uses the IRIX `$v0`/`$a3`
  convention.
- A native cell is a kernel-mode task in the shared kseg0 address space.  It enters the
  kernel on `syscall 0x1F7` and uses the convention in §5.
- The kernel's object layer (`Job`, `File`, `Socket`, `Vmo`, `Vmar`, `Port`, `Channel`)
  is common.  The compatibility layer maps IRIX syscalls onto it; a legacy process never
  sees a handle, and a native cell never sees an IRIX syscall number.
- The two conventions must never be mixed.  The kernel dispatches on the task
  personality **and** the gate instruction; a native cell that executes a plain
  `syscall` faults, and a legacy process that executes `syscall 0x1F7` faults.

---

## 2. Execution model

### 2.1 A cell is a loaded image with a capability table

A native cell is loaded from a Rust object or ELF-like image by a custom loader (D13).
At runtime it consists of:

- the image's VMARs: `.text` (read/execute), `.rodata` (read), `.data` and `.bss`
  (read/write);
- a stack VMAR;
- a read-only **start page** holding `CellStart`, the capability manifest, the fd
  array, `argv`/`envp`/`auxv` and the libc start blob;
- a per-cell capability table, owned by the kernel;
- a `Job` object, a `Namespace` object and (optionally) child objects;
- one or more `Thread` objects;
- libc state in the cell's memory: the fd table, the TCBs, the signal table, the heap.

The architecture's D3 applies: a **native process is one cell with a Unix personality**.
`fork` (where supported, see §6.5) duplicates the cell; `exec` replaces its image;
signals are events; file descriptors are handles in the libc's fd table.

### 2.2 Address space: kseg0 direct map for M9

The M9 execution model is the framekernel single-address-space (SAS) model of D8, D9 and
D17:

| Property | M9 value |
|---|---|
| Segment | kseg0, `0x8000_0000`–`0x8FFF_FFFF` (256 MiB direct map) |
| Translation | None.  kseg0 is unmapped; `vaddr = 0x8000_0000 + paddr` |
| Cell virtual address | The direct-map address of the cell's physical frame |
| Privilege | Kernel mode (`Status.KSU = kernel`), interrupts enabled |
| Page size | 4 KiB (`CellStart.page_size`); huge pages are not used for cells in v0.1 |
| Address-space switch | None; the scheduler switches tasks, not page tables |
| MMIO | kseg1 `0xA000_0000`–`0xBFFF_FFFF`, uncached |
| Reserved low window | Modules, framebuffer shadow and Seeq DMA structures; never allocated to cells |

Consequences that the rest of this document depends on:

1. **A cell's virtual address is its physical address.**  There is no per-cell page
   table to remap a virtual range to a different frame.  This is why general `fork` is
   not implementable in v0.1 (§6.5) and why `mprotect`/guard pages are metadata, not
   hardware protection (§2.3).
2. **Every mapping must be physically contiguous.**  A `VMAR` of *n* bytes needs *n*
   contiguous bytes of physical RAM, because the direct map presents the frames at their
   physical addresses.  The M9 frame allocator must provide contiguous extents; the
   maximum `mmap` size is bounded by the largest free extent.  This is a real cost of
   SAS and a constraint on the allocator.
3. **Shared memory is naturally zero-copy.**  Two cells that map the same `VMO` see the
   same physical frames, hence the same kseg0 addresses.  A `VMO` handle is the D12 bulk
   transfer mechanism; the kernel does not copy the data.
4. **The kernel and all cells are in one address space.**  A safe Rust cell cannot name
   an arbitrary address, but the hardware does not stop a cell that uses `unsafe`.  See
   §2.3.

A cell's pointers are classified by address:

| Class | Range | Cache | Backing | Allowed as a kernel buffer? |
|---|---|---|---|---|
| `MEM` | `0x8000_0000`–`0x8FFF_FFFF` | kseg0, cacheable | `VMAR` over RAM frames | Yes, after the §3.7 check |
| `DEV` | `0xA000_0000`–`0xBFFF_FFFF` | kseg1, uncached | `Mmio`/`DmaWindow` mapping | No; device methods only |
| Kernel | Everything else | — | Kernel-owned | Never accepted from a cell |

The loader allocates cell frames from the general frame allocator above the reserved low
window (the module area ends at physical `0x0860_0000`; the Seeq DMA structures follow;
the frame allocator's first frame is at `0x08B0_0000`).  The exact base is
allocator-dependent and is reported in `CellStart.image_base`, `stack_base` and the
addresses returned by `mmap`.

### 2.3 Privilege, trust and what SAS does not enforce

Native cells run in **kernel mode**.  "De-privileged" in D17 means *outside the trusted
framework and limited to the capability API*, not "less CPU privilege" and not ring 3.
The trust model has four levels:

| Code | Language | Memory safety | Authority |
|---|---|---|---|
| Privileged framework | `unsafe` Rust and assembly, minimal | The TCB | Everything |
| Core services and normal cells | Safe Rust, no `unsafe` | Rust's type system + the kernel API | Only the capabilities granted |
| Driver cells | Rust with documented `unsafe` | The driver's invariants; a bug can corrupt memory | Only the device capabilities granted |
| Legacy IRIX processes | Foreign binaries | The MMU (kuseg, ASID, user mode) | The compat container's translated view |

The capability model limits **authority**, not **memory**.  A safe Rust cell cannot forge
a pointer to another cell's memory or to device registers without `unsafe`; the kernel
API (`copyin`, `copyout`, `VMAR`, `Mmio`) validates ownership.  This is the Asterinas
argument.  It does **not** provide hardware isolation against a cell that contains
`unsafe`, and it does not provide the following:

- **No guard pages.**  kseg0 has no page protection; a stack overflow walks into
  whatever follows.  A 4 KiB gap below the stack is not in any VMAR, so the kernel
  refuses to use it as a buffer, but the cell can still read/write it.
- **No W^X.**  A cell can write its own `.text` if it uses `unsafe`.  The loader maps
  text read/execute and the kernel refuses `copyout` to an executable VMAR, but this is
  policy, not hardware.
- **No `mprotect(PROT_NONE)` enforcement.**  `VMAR::PROTECT` updates the VMAR metadata
  and the kernel's checks; direct cell accesses are unaffected.
- **No general `fork`.**  See §6.5.
- **No demand paging, no copy-on-write, no swap.**
- **No privileged-instruction trap.**  CP0 access, `tlb*`, `cache`, `eret`, `wait` and
  interrupt disable are forbidden by the ABI; a malicious or buggy cell can execute
  them.  Only trusted code is loaded as a native cell in v0.1.

This is the deliberate SAS trade-off.  The alternative — per-cell address spaces and
user mode — is the open question in §10 and the forcing function for `fork`, guard pages
and untrusted native code.

### 2.4 What is not yet decided

| Decision | This draft | Recommended default | Milestone |
|---|---|---|---|
| Per-cell address space vs SAS | SAS, kseg0 direct map | SAS for M9; mapped kseg2 or kuseg cells for untrusted code and `fork` | Post-M9.5 |
| User mode vs kernel-mode cells | Kernel mode | Kernel mode for M9; user mode only with a mapped address space | Post-M9.5 |
| Are native cells trusted? | Yes, safe Rust or reviewed driver `unsafe` | Yes for M9; no untrusted native binaries until mapped cells | M9 |
| Does a cell have a fixed address-space window? | No; direct-map addresses from the allocator | No fixed window in SAS | M9 |
| Does `fork` exist? | Contract defined, not implementable | Defer to mapped cells; use `posix_spawn`/`exec` | Post-M9.5 |
| Does `mprotect` protect? | Metadata only | Document; no security claim | M9 |
| Is the cell image ELF `ET_REL`? | Recommended | ELF32 big-endian `ET_REL` subset plus a manifest | M9 |

The M9/M9.5 split is summarised in §9.7.

### 2.5 Cell image and loader

The native image is **not** the IRIX compatibility image of [`elf.md`](elf.md).  The
compat parser deliberately rejects `ET_REL`; the native loader needs relocatable objects
for D13.  The recommended v0.1 image is:

| Property | Value |
|---|---|
| Class/data | ELF32, big-endian |
| Type | `ET_REL` (relocatable object) |
| Machine | `EM_MIPS` |
| Flags | `EF_MIPS_ABI2` (n32) and `EF_MIPS_ARCH_3` (MIPS III) |
| Sections | `.text` (AX), `.rodata` (A), `.data` (AW), `.bss` (NOBITS), `.irix7.cell` (A, manifest) |
| Symbols | `_start` is the entry unless the manifest names another; no undefined symbols |
| Relocations | A fixed subset (see below); anything else is a load error |
| Libraries | Static libc only in v0.1; no `PT_DYNAMIC`, no `dlopen` |

The loader is kernel code.  Its contract is:

1. Read the image through a `File` capability with `READ` and `EXECUTE`; verify the ELF
   header, section table, manifest magic and ABI version.
2. Allocate a `Job`, a capability table and a `Namespace` handle.  Slot 1 is the self
   `Job`; slot 2 is the `Namespace`; both get generation 1.
3. Allocate a contiguous image extent, copy the section data, zero `.bss`, apply the
   supported relocations and verify that `_start` is in an executable section.
4. Map the image VMARs (text RX, rodata R, data/bss RW), the stack (RW), a 4 KiB gap
   below the stack (not in any VMAR) and the read-only start page.
5. Build the capability manifest, the fd array, `argv`/`envp`/`auxv` and the libc start
   blob in the start page; fill `CellStart`; cross-check every handle against the table.
6. **Write back the data cache and invalidate the instruction cache** before entry.  A
   compiler fence is not enough on the R4400 (AGENTS.md).
7. Enter at `entry` with the register state in §2.6.

The supported relocation set for v0.1 is `R_MIPS_32`, `R_MIPS_26`, `R_MIPS_HI16`,
`R_MIPS_LO16` and `R_MIPS_GPREL16`; `R_MIPS_GOT16`/GOT-based code is a loader decision
in M9 (open, §10).  A load error leaves the parent's namespace unchanged and starts
nothing.

The `CellManifest` section (Appendix A.4) carries the ABI version, the entry offset, the
stack size, the minimum capability slots, the build id and the cell name.  It is
metadata, not authority.

### 2.6 Startup register state

The loader calls the cell as if it were a C function:

| Register | Value |
|---|---|
| `$a0` | Pointer to `CellStart` (a `MEM` address in the start page) |
| `$sp` | `CellStart.stack_top` (16-byte aligned) |
| `$gp` | The image's `_gp` value, or 0 |
| `$28` | 0; the libc allocates its TCB and sets `$28` before entering Rust (v0.1) |
| `$ra` | 0 |
| `$v0`, `$v1`, `$at`, `$a1`–`$a3`, `$t0`–`$t9`, `$s0`–`$s8` | 0 |
| `$k0`, `$k1` | Kernel trap scratch; the cell must not rely on them |
| `Status` | Kernel mode, `IE = 1`, `EXL = 0`, `CP1 = 0` (soft-float) |
| `EPC` | `entry` |

`_start` must never return.  A return jumps to 0 and faults; the kernel terminates the
cell with reason `BAD_ENTRY`.  The cell must not use `$k0`/`$k1` (kernel trap scratch);
the compiler does not allocate them.

---

## 3. `CellStart`

`CellStart` is the versioned, fixed-layout structure the loader delivers to `_start`.
It is **read-only cell memory** and **descriptive**: the kernel's capability table and
`Job`/`Namespace` objects are authoritative.  A cell that forges its own copy gains
nothing; the kernel cross-checks every field it uses.

### 3.1 Layout

The structure is `repr(C)`, 8-byte aligned, 160 bytes (`0xA0`) in v0.1.  All fields are
big-endian; 64-bit fields are 8-byte aligned.  Pointers are 32-bit `MEM` addresses and
obey §3.7.

```c
struct CellStart {
    uint32_t magic;             /* 0x00: 0x49583743 "IX7C" */
    uint16_t abi_major;         /* 0x04: 0 in v0.1 */
    uint16_t abi_minor;         /* 0x06: 1 in v0.1 */
    uint32_t size;              /* 0x08: 0xA0 in v0.1 */
    uint32_t flags;             /* 0x0C: 0 in v0.1 */
    uint64_t job;               /* 0x10: self Job handle */
    uint64_t namespace_handle;  /* 0x18: Namespace handle */
    uint32_t caps;              /* 0x20: CellCap[] pointer */
    uint32_t cap_count;         /* 0x24 */
    uint32_t argv;              /* 0x28: uint32_t[] of string pointers */
    uint32_t argc;              /* 0x2C */
    uint32_t envp;              /* 0x30 */
    uint32_t envc;              /* 0x34 */
    uint32_t auxv;              /* 0x38: CellAux[] pointer */
    uint32_t auxc;              /* 0x3C: number of pairs, including AT_NULL */
    uint32_t entry;             /* 0x40: entry PC (MEM, executable) */
    uint32_t gp;                /* 0x44: GOT pointer, or 0 */
    uint32_t stack_base;        /* 0x48 */
    uint32_t stack_size;        /* 0x4C */
    uint32_t stack_top;         /* 0x50 */
    uint32_t image_base;        /* 0x54 */
    uint32_t image_size;        /* 0x58 */
    uint32_t page_size;         /* 0x5C: 4096 */
    uint8_t  random[16];        /* 0x60: AT_RANDOM bytes */
    uint32_t tls_base;          /* 0x70: 0 in v0.1 (reserved for ELF TLS) */
    uint32_t tls_size;          /* 0x74: 0 in v0.1 */
    uint32_t pid;               /* 0x78 */
    uint32_t ppid;              /* 0x7C */
    uint32_t fds;               /* 0x80: CellFd[] pointer */
    uint32_t fd_count;          /* 0x84 */
    uint32_t libc_data;         /* 0x88: opaque libc start blob */
    uint32_t libc_data_size;    /* 0x8C */
    uint8_t  reserved[16];      /* 0x90: zero in v0.1 */
};
```

| Offset | Size | Align | Field | Meaning |
|---:|---:|---:|---|---|
| 0x00 | 4 | 4 | `magic` | `0x49583743` (`IX7C`) |
| 0x04 | 2 | 2 | `abi_major` | 0 for the v0 line |
| 0x06 | 2 | 2 | `abi_minor` | 1 for this document |
| 0x08 | 4 | 4 | `size` | Total bytes; 160 here |
| 0x0C | 4 | 4 | `flags` | Reserved; zero in v0.1 |
| 0x10 | 8 | 8 | `job` | Self `Job`; must equal slot 1 (§3.2) |
| 0x18 | 8 | 8 | `namespace_handle` | `Namespace`; must equal slot 2 |
| 0x20 | 4 | 4 | `caps` | `CellCap[]`, `cap_count * 24` bytes |
| 0x24 | 4 | 4 | `cap_count` | Includes slots 1 and 2; 2..4096 |
| 0x28 | 4 | 4 | `argv` | `uint32_t[]`, `argc + 1` entries, last 0 |
| 0x2C | 4 | 4 | `argc` | 0..4096 |
| 0x30 | 4 | 4 | `envp` | `uint32_t[]`, `envc + 1` entries, last 0 |
| 0x34 | 4 | 4 | `envc` | 0..4096 |
| 0x38 | 4 | 4 | `auxv` | `CellAux[]`, `auxc` pairs |
| 0x3C | 4 | 4 | `auxc` | Includes the `AT_NULL` pair; 1..64 |
| 0x40 | 4 | 4 | `entry` | Entry PC; executable VMAR; 4-aligned |
| 0x44 | 4 | 4 | `gp` | `$gp` for the image, or 0 |
| 0x48 | 4 | 4 | `stack_base` | Lowest stack address |
| 0x4C | 4 | 4 | `stack_size` | At least 64 KiB |
| 0x50 | 4 | 4 | `stack_top` | `stack_base + stack_size`; 16-aligned |
| 0x54 | 4 | 4 | `image_base` | Lowest image address |
| 0x58 | 4 | 4 | `image_size` | Image extent in bytes |
| 0x5C | 4 | 4 | `page_size` | 4096 |
| 0x60 | 16 | 1 | `random` | 16 random bytes; `AT_RANDOM` points here |
| 0x70 | 4 | 4 | `tls_base` | Must be 0 in v0.1 |
| 0x74 | 4 | 4 | `tls_size` | Must be 0 in v0.1 |
| 0x78 | 4 | 4 | `pid` | Process id; positive |
| 0x7C | 4 | 4 | `ppid` | Parent pid; 0 for init |
| 0x80 | 4 | 4 | `fds` | `CellFd[]`, `fd_count * 16` bytes |
| 0x84 | 4 | 4 | `fd_count` | 0..1024 |
| 0x88 | 4 | 4 | `libc_data` | Opaque libc start blob |
| 0x8C | 4 | 4 | `libc_data_size` | 0..64 KiB |
| 0x90 | 16 | 1 | `reserved` | Zero in v0.1 |

The C definition is the readable form; the table is authoritative for offsets.  The
`native-abi` crate must assert `size_of::<CellStart>() == 160`,
`align_of::<CellStart>() == 8` and every offset above.

```c
#define CELL_START_MAGIC    0x49583743u   /* "IX7C" */
#define CELL_ABI_MAJOR      0
#define CELL_ABI_MINOR      1
#define CELL_START_SIZE     0xA0u

#define CELL_DEFAULT_CAPS   256
#define CELL_MAX_CAPS       4096
#define CELL_DEFAULT_FDS    256
#define CELL_MAX_FDS        1024
#define CELL_MAX_BLOCK      4096
#define CELL_MAX_IO         (1024u * 1024u)
#define CELL_PATH_MAX       1024
#define CELL_NAME_MAX       255
#define CELL_ARG_MAX        (128u * 1024u)
#define CELL_MAX_HANDLES    8
```

### 3.2 `CellCap` and well-known slots

```c
struct CellCap {
    uint64_t handle;   /* 0x00 */
    uint32_t type;     /* 0x08: CellObjType */
    uint32_t rights;   /* 0x0C: CellRights */
    char     name[8];  /* 0x10: NUL-padded ASCII, at most 7 characters */
};                     /* 0x18 = 24 bytes, 8-aligned */
```

The array is sorted by ascending slot index.  It is a **manifest for discovery**, not
authority: the kernel's table is authoritative.  A cell may use `CellCap` to find its
standard descriptors, network device, root namespace or parent `Job` by name.

Well-known slots:

| Slot | Generation | Object | Name | Rights (recommended) |
|---:|---:|---|---|---|
| 0 | 0 | null | — | Never valid |
| 1 | 1 | self `Job` | `self` | `INVOKE`, `READ`, `WRITE`, `MANAGE`, `DUP`, `TRANSFER`, `REVOKE` |
| 2 | 1 | `Namespace` | `ns` | `INVOKE`, `READ`, `WRITE`, `MANAGE` |
| 3 | 1+ | stdin `File` (when present) | `stdin` | `READ`, `DUP`, `TRANSFER` |
| 4 | 1+ | stdout `File` (when present) | `stdout` | `WRITE`, `DUP`, `TRANSFER` |
| 5 | 1+ | stderr `File` (when present) | `stderr` | `WRITE`, `DUP`, `TRANSFER` |
| 6+ | 1+ | loader-granted | `root`, `net`, … | Attenuated from the parent |

The loader allocates slots 1 and 2 first, so their generation is 1 and the handles are
stable constants:

```c
#define CELL_SELF_HANDLE      0x0000000100000001ULL
#define CELL_NAMESPACE_HANDLE 0x0000000100000002ULL
```

`CellStart.job` and `CellStart.namespace_handle` must equal these values in v0.1.  The
fields exist (rather than being implicit) so a future loader can choose a different
layout without changing the struct.

Slot 1 is special across `fork`: the child's copy is **retargeted** to the child `Job`
while keeping the same index and generation, so the handle bits stay valid (§6.5).  Slot
2 is shared or retargeted in the same way.  A cell must not revoke or reuse slots 1 and
2; only the kernel's lifecycle code does that.

### 3.3 `CellFd`

```c
struct CellFd {
    int32_t  fd;        /* 0x00: 0..1023 */
    uint32_t flags;     /* 0x04: CELL_FD_CLOEXEC = 1, CELL_FD_CLOFORK = 2 */
    uint64_t handle;    /* 0x08: a File/Socket/Device handle in this cell */
};                       /* 0x10 = 16 bytes, 8-aligned */
```

`CellStart.fds` describes the initial file descriptors, in ascending `fd` order.
`fd_count` is zero when the cell has no descriptors.  Every `handle` must appear in the
`CellCap` array and must be a file-like object (`File`, `Socket` or `Device`).

The array is an **input** to `Job::SPAWN`/`Job::EXEC` (in the parent's handle space) and
an **output** in the child's `CellStart` (in the child's handle space).  The loader
rewrites the child's array with the child's handles; the parent's array is untouched.
See §6.6 and §7.5.  Duplicate fd entries may refer to the same handle; the libc
deduplicates them and shares one reference count.

### 3.4 Auxiliary vector

`auxv` points to an array of `CellAux` pairs, terminated by `AT_NULL`:

```c
struct CellAux {
    uint32_t type;   /* 0x00 */
    uint32_t value;  /* 0x04: scalar or MEM pointer, depending on type */
};                    /* 8 bytes */
```

| Key | Value | Meaning |
|---:|---|---|
| 0 | 0 | `AT_NULL`; terminator |
| 1 | 1 | `AT_PAGESZ`; must equal `page_size` |
| 2 | 2 | `AT_ENTRY`; must equal `entry` |
| 3 | 3 | `AT_PHDR`; 0 for the native loader in v0.1 |
| 4 | 4 | `AT_PHNUM`; 0 in v0.1 |
| 5 | 5 | `AT_BASE`; 0 for a static cell |
| 6 | 6 | `AT_RANDOM`; pointer to `CellStart.random` |
| 7 | 7 | `AT_EXECFN`; pointer to the image path (NUL-terminated) |
| 8 | 8 | `AT_UID`; label, no authority |
| 9 | 9 | `AT_EUID` |
| 10 | 10 | `AT_GID` |
| 11 | 11 | `AT_EGID` |
| 12 | 12 | `AT_HWCAP`; MIPS feature bits (open) |
| 13 | 13 | `AT_CLKTCK`; 100 |
| 14 | 14 | `AT_SECURE`; 0, always |
| 15 | 15 | `AT_PLATFORM`; pointer to `"mips"` (optional) |

The set is a recommended default and remains open (§10).  Unknown keys are ignored by
readers; a pointer-valued key that the loader does not know is not validated.  `AT_SECURE`
is always 0: there are no setuid cells and no privilege gain on exec.

`CellStart` is the authoritative source for `argv`, `envp` and `auxv`; the loader need
not duplicate them on the initial stack.  The libc's `_start` may build the conventional
System V stack shape from these fields for a C runtime, but `std` and the native libc read
`CellStart` directly.  This keeps one copy of the startup data and avoids a second
layout that could disagree with the struct.

### 3.5 Versioning rule

`CellStart` is versioned by `abi_major`/`abi_minor` and self-describing by `size`.

- **Major** changes are incompatible: field movement, a changed meaning, a removed
  field, a changed handle encoding.  A v0 kernel refuses a cell with a non-zero major;
  a v1 libc refuses a v0 kernel.
- **Minor** changes are additive: a new field is appended at the end, or consumes the
  `reserved` area, and `size` grows only when the reserved area is exhausted.  A reader
  must check `size` before reading a field and must not read beyond `size`.  A writer
  must zero any field it does not set.
- **Reserved** bytes are zero in v0.1.  A v0.1 kernel rejects a non-zero reserved field
  when `abi_minor == 1`.  A future minor may define them; the loader is responsible for
  validating them before the kernel sees them.
- **Flags** are zero in v0.1.  An unknown flag bit is a load error, not an ignored bit,
  because a flag may change the meaning of the rest of the structure.
- **No silent growth.**  A new field that does not fit the reserved area bumps the minor
  and grows `size`; a kernel that does not know the minor refuses the cell if it needs
  the field.  The libc checks the same fields against its own minimum.

The same principle applies to invocation blocks (§4.5): `size` is the layout, `version`
is the semantics, and existing offsets never move.

### 3.6 Validation checklist

The loader and the kernel perform these checks **before the first instruction**.  A
failure refuses the load; a partially initialised cell never runs.  Checks are listed in
order; later checks may assume earlier ones.

1. `magic == 0x49583743`.
2. `abi_major == 0`; `abi_minor <= CELL_ABI_MINOR` (1 in v0.1).
3. `size >= 0xA0`, `size % 8 == 0`, `size <= 4096`, and the `CellStart` range is inside
   one readable `MEM` VMAR.
4. `flags == 0`; `reserved[16]` is all zero (for `abi_minor == 1`).
5. `job` resolves to slot 1, type `Job`, with `INVOKE`; `job == CELL_SELF_HANDLE`.
6. `namespace_handle` resolves to slot 2, type `Namespace`, with `INVOKE`;
   `namespace_handle == CELL_NAMESPACE_HANDLE`.
7. `caps != 0`, `cap_count >= 2`, `cap_count <= 4096`; the array range
   `cap_count * 24` is readable, 8-aligned and inside one `MEM` VMAR.
8. Every `CellCap` handle resolves in the table, its `type` matches the table, its
   `rights` are a subset of the table entry's rights, its name is NUL-terminated within
   8 bytes and is ASCII.
9. The `CellCap` array contains slots 1 and 2 exactly once and is sorted by slot index.
10. `argv`/`argc`: if `argc == 0`, `argv` is 0 or points to a single 0; otherwise
    `argv` points to `argc + 1` 4-byte entries, the last is 0, every string pointer is in
    a readable `MEM` VMAR, every string is NUL-terminated within 4096 bytes, and the
    total is at most 128 KiB.
11. `envp`/`envc`: the same rules; `envc == 0` is allowed.
12. `auxv`/`auxc`: `auxc >= 1`, `auxc <= 64`, the array is `auxc * 8` bytes, 4-aligned,
    readable, the last pair is `(AT_NULL, 0)`, `AT_PAGESZ == page_size` and
    `AT_ENTRY == entry`.  If present, `AT_RANDOM` points to a readable 16-byte range and
    `AT_EXECFN` points to a readable NUL-terminated string.
13. `entry != 0`, `entry % 4 == 0`, and `entry` is inside an executable `MEM` VMAR.
14. `gp == 0` or `gp` is inside a readable `MEM` VMAR; `gp % 4 == 0`.
15. `stack_size >= 65536`, `stack_base % 4 == 0`, `stack_top == stack_base +
    stack_size`, `stack_top % 16 == 0`, and `stack_base .. stack_top` is inside one
    writable, non-executable `MEM` VMAR.
16. `image_size > 0`, `image_base % 4 == 0`, `image_base .. image_base + image_size` is
    inside readable `MEM` VMARs, and `entry` is inside the image extent.
17. `page_size == 4096`.
18. `tls_base == 0` and `tls_size == 0` in v0.1.
19. `pid > 0`, `ppid >= 0`, `pid != ppid`, and the values match the `Job`'s recorded ids.
20. `fds`/`fd_count`: if `fd_count == 0`, `fds == 0`; otherwise `fd_count <= 1024`, the
    range `fd_count * 16` is readable and 8-aligned, each `fd` is unique and in
    `0..1023`, each `flags` has only `CELL_FD_CLOEXEC|CELL_FD_CLOFORK`, and each
    `handle` resolves in the table to a `File`, `Socket` or `Device`.
21. `libc_data`/`libc_data_size`: if the size is 0, the pointer is 0; otherwise the
    range is readable, at most 64 KiB, and 4-aligned.  The kernel does not interpret it.
22. Every pointer passes the §3.7 address rule; every multiplication and addition is
    checked for 32-bit overflow; every count is bounded before a loop.

A loader bug that produces an inconsistent structure is a kernel bug; the kernel must
refuse the cell rather than trust it.  The checks are cheap relative to loading, and they
run once per cell.

### 3.7 Address rule for cell pointers

Every pointer a cell passes to the kernel is a 32-bit address in one of the two classes
in §2.2.  The kernel resolves it before dereferencing:

1. **Class check.**  A normal buffer is `MEM` (`0x8000_0000..0x8FFF_FFFF`); a device
   buffer is `DEV` (`0xA000_0000..0xBFFF_FFFF`).  A pointer outside both is `E_FAULT`.
   `copyin`/`copyout` accept only `MEM`; `Mmio`/`DmaWindow` methods accept `DEV` where
   documented.
2. **Ownership check.**  The range must lie inside one `VMAR` owned by the calling cell
   (the kernel keeps an ordered VMAR list per cell) or, for a shared `VMO`, inside a
   `VMAR` the cell has mapped.  The kernel does not accept a pointer just because it is
   in kseg0.
3. **Direction check.**  `copyin` requires `READ`; `copyout` requires `WRITE`;
   `EXECUTE` is required for an entry point; `MAP` is required to create a mapping.
4. **Range check.**  `ptr + len` is computed with checked 32-bit arithmetic; the range
   must not wrap and must not cross a VMAR boundary.  Callers that need a large buffer
   pass a `VMO` handle instead of a scatter of VMARs.
5. **Reference.**  The kernel holds a reference on the `VMAR`/`VMO` for the duration of
   the invocation.  A concurrent `VMAR::UNMAP` of a range in use returns `E_BUSY` (or
   waits); the kernel never dereferences a pointer whose mapping has gone.
6. **No kernel pointers.**  No cell-visible structure contains a kernel address.  The
   kernel's `Cell`/`Job`/`VMAR` structs are separate from the ABI structs.  A pointer
   from a cell is always interpreted in the cell's address space.
7. **Read-only metadata.**  `CellStart`, `CellCap[]`, `CellFd[]`, `argv`/`envp`/`auxv`
   and `libc_data` live in a read-only start page.  The kernel writes them before entry;
   the cell treats them as immutable.
8. **Device pointers.**  An `Mmio` mapping is a `DEV` range; the kernel never copies it.
   A driver accesses it directly.  A `DmaWindow` mapped `CACHE_NONE` is `DEV`; one
   mapped `CACHE_WB` is `MEM` and requires explicit `SYNC`.
9. **Futex keys.**  For shared-memory futexes (§4.6), the kernel derives the key from
   the **physical** address of the word (the `VMAR`/`VMO` translation), not the virtual
   address, so two cells that share a `VMO` wait on the same key.

The rule protects the **kernel** from being a confused deputy.  It does not protect one
cell from another; that is the SAS trade-off in §2.3.

---

## 4. Capabilities, objects and invocation

### 4.1 Handles

A handle is a 64-bit scalar:

```text
 63          32 31           0
+--------------+--------------+
|  generation  |    index     |
+--------------+--------------+
```

```c
typedef uint64_t cell_handle_t;

#define CELL_HANDLE_NULL       0
#define CELL_HANDLE_INDEX(h)   ((uint32_t)((h) & 0xFFFFFFFFu))
#define CELL_HANDLE_GEN(h)     ((uint32_t)((h) >> 32))
#define CELL_HANDLE(index, g)  (((uint64_t)(uint32_t)(g) << 32) | (uint32_t)(index))
```

- Index 0 is reserved for the null handle; a valid table slot is 1..`max_index`.
- Generation 0 is invalid.  A slot's first allocation sets generation 1; each reuse
  increments it and skips 0 on wrap.
- The null handle is `0`; it is accepted only for system invocations (type 0), where a
  zero handle is required.
- **Rights are not in the handle.**  They live in the table entry and are reported in
  `CellCap` for discovery.  A cell cannot forge or amplify rights by changing a handle.
- The ABI allows up to `0x000F_FFFF` (1,048,575) slots; v0.1 defaults to 256 and caps
  at 4096 (§8.3).  The kernel returns `E_NOMEM` when the table is full.
- Every use validates: index in range, slot allocated, generation match, object type
  match and the requested rights.  A stale, revoked or forged handle is `E_BADF`; it is
  never dereferenced.

### 4.2 Rights

| Bit | Name | Meaning |
|---:|---|---|
| 0 | `READ` | Read the object's data or receive from it |
| 1 | `WRITE` | Write data, send, or mutate state |
| 2 | `EXECUTE` | Use as an executable image or search a directory |
| 3 | `MAP` | Create, change or remove a mapping |
| 4 | `DMA` | Use a DMA window or a device's DMA path |
| 5 | `INVOKE` | Perform the object's ordinary methods |
| 6 | `TRANSFER` | Copy the capability to another cell |
| 7 | `DUP` | Duplicate or derive the capability in this cell |
| 8 | `REVOKE` | Recursively invalidate derived capabilities |
| 9 | `MANAGE` | Create/destroy children, change quotas, set attributes |

Bits 10–30 are reserved and must be zero in a `CellCap` or a `CAP_DERIVE` request.
Bit 31 is kernel-only and is never granted to a cell.  Derivation is attenuating only:
the requested rights must be a subset of the parent's.  A `CellCap` that claims more
rights than the table is ignored; the table wins.

### 4.3 Object types

| Type | Object | Purpose | Key rights |
|---:|---|---|---|
| 0 | `System` | Pseudo-type for authority-free system invocations | — |
| 1 | `Job` | Cell/process group, resource accounting, revocation root, child creation | `INVOKE`, `MANAGE`, `READ`, `REVOKE` |
| 2 | `Channel` | Synchronous IPC endpoint pair with handle transfer | `READ`, `WRITE`, `INVOKE`, `TRANSFER` |
| 3 | `Port` | Event queue: signals, child exits, readiness, IRQs | `READ`, `WRITE`, `INVOKE` |
| 4 | `Vmar` | Virtual memory region: map/unmap/protect | `MAP`, `READ`, `WRITE`, `EXECUTE` |
| 5 | `Vmo` | Memory object backing a VMAR or a DMA/shared buffer | `READ`, `WRITE`, `MAP`, `DMA` |
| 6 | `File` | VFS object: regular file, directory, device file, pipe, synthetic file | `READ`, `WRITE`, `EXECUTE`, `MAP`, `MANAGE` |
| 7 | `Socket` | Network endpoint | `READ`, `WRITE`, `INVOKE`, `TRANSFER` |
| 8 | `Device` | Device instance: MMIO, IRQ and DMA parent; socket factory | `INVOKE`, `MANAGE`, `DMA` |
| 9 | `Mmio` | A mapped MMIO range | `MAP`, `READ`, `WRITE` |
| 10 | `Irq` | An interrupt line delivered as an event | `READ`, `INVOKE` |
| 11 | `DmaWindow` | A physically contiguous, cache-managed DMA buffer | `DMA`, `READ`, `WRITE`, `MAP` |
| 12 | `Namespace` | Mount/bind tree with a root `File` | `INVOKE`, `MANAGE` |
| 13 | `Thread` | A task in the cell | `INVOKE`, `READ`, `MANAGE` |

Types 14–`0xFFFF` are reserved.  `Namespace` and `Thread` extend the object list in
architecture §5.2; they are separate objects because they have their own lifecycle and
rights.  `Mmio`, `Irq` and `DmaWindow` are child objects of `Device` so that a device
grant can be revoked as a unit.

### 4.4 Derivation, delegation and revocation

**Derivation.**  `Job::CAP_DERIVE(handle, rights)` creates a new slot that refers to the
same object with a subset of the parent's rights.  It requires `DUP` on the parent.
`Job::CAP_DUP(handle)` is the same with the parent's rights.  The new capability records
the parent slot as its derivation parent.

**Drop.**  `Job::CAP_DROP(handle)` removes the caller's slot.  Descendants in the
caller's table that were derived from it are dropped too; copies in other cells are
unaffected.  Dropping authority is always allowed, even without `DUP`.  When the last
capability to an object is dropped, the object is destroyed and its destructor runs in
the kernel.

**Delegation.**  `Channel::SEND`/`RECV` and `CALL` carry up to eight handles.  The sender
must hold `TRANSFER` on each; the receiver gets a new capability with the same rights
(or fewer, if the channel's transfer policy attenuates).  The transfer is atomic: if the
receiver's table is full, the whole send fails with `E_NOMEM` and no handle is copied.
The derivation parent of the receiver's capability is the sender's capability, so a
later `REVOKE` by the sender invalidates the receiver's copy.

**Revocation.**  `Job::CAP_REVOKE(handle)` requires `REVOKE` on the handle.  It walks
the derivation subtree — across cells, not just the caller's table — and invalidates
every descendant: the slot is freed, the generation is incremented, and any waiter
blocked on the object returns `E_REVOKED`.  The handle becomes `E_BADF`.  Revocation is
not "close": it is a tree operation, and it is the reason handles carry generations.

A `Device` capability's children (`Mmio`, `Irq`, `DmaWindow`) record the `Device` cap as
their parent, so revoking the device grant also revokes its MMIO, IRQ and DMA children.
This is the model for "a parent can revoke a child's device capability, and the device
cell must stop using it" (architecture §5.8).

**Exit.**  When a `Job` exits, its capability table is dropped, its child objects are
destroyed in dependency order, its child `Job`s are reparented to `init` (pid 1) or
killed (open, §10), and its exit status is kept for `Job::WAIT`.  Other cells' handles
to the dead `Job` remain valid but return `E_REVOKED`/`E_CHILD`; a `Job` cap is not a
liveness guarantee.

### 4.5 Invocation encoding

The native gate is `syscall 0x1F7` (instruction word `0x0000_7DCC`).  It is a distinct
20-bit code in the MIPS `syscall` instruction; IRIX 6.5 uses code 0.  The kernel
dispatches on the task personality and the code:

- native cell + code `0x1F7` → native invocation;
- legacy process + code 0 → IRIX syscall;
- native cell + code 0, or legacy process + code `0x1F7` → fault (`BAD_TRAP` for the
  cell, `SIGSYS`/`SIGILL` for the legacy process).

The gate must not be placed in a branch delay slot; the libc wrapper emits a standalone
instruction and the kernel returns to `EPC + 4`.  `$k0`/`$k1` are kernel trap scratch and
must not be used by a cell.

Register convention:

| Register | On entry | On return |
|---|---|---|
| `$v0` | Selector: `(type << 16) \| method` | Status (`CELL_OK` or `CELL_E_*`) |
| `$a0` | Handle (64-bit; 0 for system invocations) | Preserved |
| `$a1` | Argument block pointer (`MEM`, 8-aligned; 0 if `size == 0`) | Preserved |
| `$a2` | Argument block size in bytes (0..4096) | Preserved |
| `$a3` | Argument block version (1 in v0.1) | Preserved |
| `$v1` | — | 64-bit result value; 0 on error |
| Other GPRs | — | Preserved |

All 32-bit arguments are zero-extended in their 64-bit registers; a non-zero upper half
is `E_INVAL`.  The block pointer must be 8-aligned; the block size must be 4-aligned and
at most 4096 bytes.  The kernel copies the block into a per-task 4 KiB scratch buffer
(no allocation on the trap path), validates `size`/`version`/reserved fields, executes,
and copies the block back.  Fields beyond `size` are not touched; a method must not write
past the caller's `size`.  A larger block from a newer caller is accepted if the method
knows the trailing fields; otherwise the extra bytes are preserved unchanged.

The selector is a 32-bit value:

```c
#define CELL_INVOKE(type, method) (((uint32_t)(type) << 16) | (uint32_t)(method))
```

Method 0 is reserved and returns `E_INVAL`.  A type mismatch between the selector and
the handle's object type is `E_INVAL`; an unknown method is `E_NOTSUP`; an unknown object
type is `E_INVAL`.  System invocations use type 0 and handle 0; they are the only calls
that require no capability (they must be authority-free, §4.6.1).

### 4.6 Method tables

The tables below are the v0.1 surface.  `No.` is the method number; `Args` names the
block fields (all blocks start at offset 0); `Rights` is the minimum required on the
handle; `Result` is `$v1` unless an out block is named.  Methods marked *M9.5* are not
required for the M9 demo.  The detailed blocks for the POSIX-critical methods follow the
tables; the rest are summarised and will be pinned by the `native-abi` crate.

#### 4.6.1 System invocations (type 0, handle 0)

| No. | Method | Args | Result |
|---:|---|---|---|
| 1 | `ABI_INFO` | — | block: `{ abi_major, abi_minor, page_size, ncpu, clock_hz }` |
| 2 | `CLOCK_GETTIME` | `{ i32 clock_id; u32 reserved }` | nanoseconds (u64) |
| 3 | `CLOCK_GETRES` | `{ i32 clock_id; u32 reserved }` | nanoseconds per tick |
| 4 | `GETRANDOM` | `{ u32 buf; u32 len }` | bytes written |
| 5 | `SLEEP` | `{ u64 deadline_ns }` | 0; `E_INTR` on a signal |
| 6 | `YIELD` | — | 0 |
| 7 | `SYSCONF` | `{ i32 name; u32 reserved }` | value (u64) |
| 8 | `UNAME` | `{ u32 buf; u32 len }` | bytes written |
| 9 | `FUTEX_WAIT` | `{ u32 addr; u32 expected; u64 deadline }` | 0; `E_AGAIN`/`E_INTR`/`E_TIMEDOUT` |
| 10 | `FUTEX_WAKE` | `{ u32 addr; u32 count }` | threads woken |
| 11 | `DEBUG_WRITE` | `{ u32 buf; u32 len }` | bytes written (kernel console, best effort) |

These calls are authority-free by construction: they read the clock, fill randomness,
sleep, yield, query system limits, uname, wait on a cell-owned futex word, or write a
diagnostic.  Adding a system invocation requires a review that proves it grants no
authority over another cell's objects.  `DEBUG_WRITE` is a development aid, rate-limited
by the kernel console; a production build may return `E_NOTSUP`.

`ABI_INFO` returns a 24-byte block `{ u16 abi_major; u16 abi_minor; u32 page_size;
u32 ncpu; u32 clock_hz; u32 reserved[2] }`.  Clock ids follow the native libc:
0 `CLOCK_REALTIME`, 1 `CLOCK_MONOTONIC`, 2 `CLOCK_PROCESS_CPUTIME_ID`,
3 `CLOCK_THREAD_CPUTIME_ID`, 4 `CLOCK_MONOTONIC_RAW`, 5 `CLOCK_REALTIME_COARSE`,
6 `CLOCK_MONOTONIC_COARSE`, 7 `CLOCK_BOOTTIME`.  `SYSCONF` names are 1 `_SC_PAGESIZE`,
2 `_SC_NPROCESSORS_ONLN`, 3 `_SC_HOST_NAME_MAX`, 4 `_SC_GETPW_R_SIZE_MAX`,
5 `_SC_ARG_MAX`, 6 `_SC_CHILD_MAX`, 7 `_SC_OPEN_MAX`, 8 `_SC_CLK_TCK`,
9 `_SC_MONOTONIC_CLOCK`, 10 `_SC_PHYS_PAGES`, 11 `_SC_AVPHYS_PAGES`.

#### 4.6.2 `Job` (type 1)

| No. | Method | Args | Rights | Result |
|---:|---|---|---|---|
| 1 | `EXIT` | `{ i32 status; u32 flags }` | `INVOKE` | does not return (self only) |
| 2 | `SIGNAL` | `{ i32 sig; u32 reserved }` | `INVOKE` | 0 |
| 3 | `KILL` | — | `INVOKE` | 0; target terminates |
| 4 | `STOP` | — | `INVOKE` | 0 |
| 5 | `CONT` | — | `INVOKE` | 0 |
| 6 | `WAIT` | `CellJobWaitIn` | `READ` | `CellJobWaitOut` |
| 7 | `GET_INFO` | `{ u32 kind; u32 buf; u32 len; u32 reserved }` | `READ` | bytes |
| 8 | `FORK` | `CellJobForkOut` | `MANAGE` | `CellJobForkOut`; `E_NOTSUP` in v0.1 (§6.5) |
| 9 | `SPAWN` | `CellJobSpawn` | `MANAGE` + image `EXECUTE` | child handle; block |
| 10 | `EXEC` | `CellJobExec` | `MANAGE` + image `EXECUTE` | does not return on success |
| 11 | `CAP_DERIVE` | `{ u64 handle; u32 rights; u32 reserved }` | `DUP` on target | new handle |
| 12 | `CAP_DUP` | `{ u64 handle }` | `DUP` on target | new handle |
| 13 | `CAP_DROP` | `{ u64 handle }` | — | 0 |
| 14 | `CAP_REVOKE` | `{ u64 handle }` | `REVOKE` on target | 0 |
| 15 | `CAP_INFO` | `{ u64 handle; u32 buf; u32 len }` | — | one `CellCap` record |
| 16 | `VMAR_CREATE` | `{ u32 size; u32 flags }` | `MANAGE` | `Vmar` handle |
| 17 | `VMO_CREATE` | `{ u32 size; u32 flags }` | `MANAGE` | `Vmo` handle |
| 18 | `CHANNEL_CREATE` | — | `MANAGE` | `{ a, b }` handles |
| 19 | `PORT_CREATE` | `{ u32 flags }` | `MANAGE` | `Port` handle |
| 20 | `THREAD_CREATE` | `CellThreadCreate` | `MANAGE` | `Thread` handle |
| 21 | `NAMESPACE_CREATE` | `{ u64 root; u32 flags; u32 reserved }` | `MANAGE` + root `READ\|EXECUTE` | `Namespace` handle |
| 22 | `PIPE_CREATE` | `{ u32 flags }` | `MANAGE` | `{ read, write }` `File` handles |
| 23 | `SET_EVENT_PORT` | `{ u64 port }` | `MANAGE` | 0 |
| 24 | `SET_PGID` | `{ u32 pgid; u32 reserved }` | `MANAGE` | 0 (M9.5) |
| 25 | `GET_PGID` | — | `READ` | pgid (M9.5) |
| 26 | `SETSID` | — | `MANAGE` | sid (M9.5) |
| 27 | `SET_IDS` | `{ u32 uid; u32 gid; u32 reserved }` | `MANAGE` | 0 (M9.5) |

`EXIT` is self-only: `$a0` must be the caller's self `Job`, or the call returns
`E_INVAL`.  Its flags are bit 0 `CELL_EXIT_PANIC` and bit 1 `CELL_EXIT_ABORT`; the kernel
records the reason and the parent's `WAIT` reports it.  `KILL` is uncatchable: it returns
0 to the caller after the target `Job` and all its threads terminate.  `STOP`/`CONT` are
job control and are uncatchable like `SIGSTOP`/`SIGCONT`.

`GET_INFO` kinds: 1 pid, 2 ppid, 3 ids (`{ uid, gid, euid, egid }`), 4 usage
(`{ cpu_ns, mem_bytes, cap_count, thread_count }`), 5 state.  The ids are labels, not
authority.

#### 4.6.3 `Namespace` (type 12)

| No. | Method | Args | Rights | Result |
|---:|---|---|---|---|
| 1 | `OPEN` | `CellNamespaceOpen` | `INVOKE` + dir `READ\|EXECUTE` | `File` handle |
| 2 | `STAT` | `CellNamespaceStat` | `INVOKE` + dir `READ\|EXECUTE` | `CellFileStat` |
| 3 | `UNLINK` | `{ u64 dir; u32 path; u32 path_len; u32 flags; u32 reserved }` | `INVOKE` + dir `WRITE` | 0 |
| 4 | `MKDIR` | `{ u64 dir; u32 path; u32 path_len; u32 mode; u32 reserved }` | `INVOKE` + dir `WRITE` | 0 |
| 5 | `RMDIR` | `{ u64 dir; u32 path; u32 path_len; u32 reserved }` | `INVOKE` + dir `WRITE` | 0 |
| 6 | `RENAME` | `CellNamespaceRename` | `INVOKE` + both dirs `WRITE` | 0 |
| 7 | `LINK` | `CellNamespaceRename` | `INVOKE` + both dirs `WRITE` | 0 |
| 8 | `SYMLINK` | `{ u64 dir; u32 path; u32 path_len; u32 target; u32 target_len }` | `INVOKE` + dir `WRITE` | 0 |
| 9 | `READLINK` | `{ u64 dir; u32 path; u32 path_len; u32 buf; u32 buf_len }` | `INVOKE` + dir `READ` | bytes |
| 10 | `BIND` | `CellNamespaceBind` | `MANAGE` + source `READ\|EXECUTE` | 0 |
| 11 | `UNBIND` | `{ u32 path; u32 path_len; u32 flags; u32 reserved }` | `MANAGE` | 0 |
| 12 | `PATH_OF` | `{ u64 file; u32 buf; u32 buf_len }` | `INVOKE` + file `READ` | bytes (M9.5) |
| 13 | `CHROOT` | `{ u64 root; u32 flags; u32 reserved }` | `MANAGE` + root `READ\|EXECUTE` | new `Namespace` handle |

`dir == 0` means the namespace root.  `path` is a `MEM` pointer to a byte string,
`path_len` excludes the terminating NUL.  `OPEN` flags are the native `O_*` subset;
`O_CREAT` requires `WRITE` on the directory.  `CHROOT` returns a new namespace rather
than mutating the current one; the libc swaps its handle.

#### 4.6.4 `File` (type 6)

| No. | Method | Args | Rights | Result |
|---:|---|---|---|---|
| 1 | `READ` | `CellFileIo` (`CELL_IO_AT` optional) | `READ` | bytes |
| 2 | `WRITE` | `CellFileIo` (`CELL_IO_AT` optional) | `WRITE` | bytes |
| 3 | `SEEK` | `{ i64 offset; u32 whence; u32 reserved }` | `READ` or `WRITE` | new offset |
| 4 | `STAT` | `{ u32 buf; u32 len }` | `READ` | `CellFileStat` |
| 5 | `INFO` | `{ u32 buf; u32 len }` | `READ` | `CellFileInfo` |
| 6 | `TRUNCATE` | `{ u64 size }` | `WRITE` | 0 |
| 7 | `SYNC` | `{ u32 flags; u32 reserved }` | `WRITE` | 0 |
| 8 | `GET_VMO` | `{ u64 offset; u32 len; u32 flags }` | `READ`/`MAP` | `Vmo` handle |
| 9 | `READDIR` | `CellFileIo` (`offset` is the cookie) | `READ` | bytes of `CellDirEntry` |
| 10 | `READLINK` | `{ u32 buf; u32 len; u32 reserved }` | `READ` | bytes |
| 11 | `SET_ATTR` | `CellFileAttr` | `MANAGE` | 0 |
| 12 | `POLL` | `{ u64 port; u64 key; u32 events; u32 reserved }` | `READ` | revents |
| 13 | `DEVCTL` | `CellFileDevctl` | `INVOKE` | bytes |
| 14 | `STATVFS` | `{ u32 buf; u32 len }` | `READ` | `CellStatVfs` (M9.5) |
| 15 | `LOCK` | `{ i32 type; i32 whence; i64 start; i64 len; u32 flags; u32 reserved }` | `READ`/`WRITE` | 0 (M9.5) |
| 16 | `UNLOCK` | `{ i32 whence; i64 start; i64 len; u32 flags }` | `READ`/`WRITE` | 0 (M9.5) |

`READ`/`WRITE` use `CellFileIo.offset` when `flags & CELL_IO_AT`; otherwise they use and
advance the **open file description's** offset in the `File` object.  The offset lives in
the kernel so that `dup`, `fork` and `posix_spawn` share it as POSIX requires.  `SEEK`
changes it.  A pipe or socket returns `E_SPIPE` from `SEEK`.  `O_APPEND` is a `File`
flag; the kernel appends under the same lock.

`GET_VMO` returns a `Vmo` that references the file's page cache.  `MAP_SHARED` maps that
VMO; `MAP_PRIVATE` makes an eager copy in the SAS model (§7.4).  `SYNC` flushes dirty
pages.

#### 4.6.5 `Vmar` (type 4) and `Vmo` (type 5)

| Object | No. | Method | Args | Rights | Result |
|---|---:|---|---|---|---|
| `Vmar` | 1 | `MAP` | `CellVmarMap` | `MAP` + VMO `MAP` | address (u32) |
| `Vmar` | 2 | `UNMAP` | `{ u32 addr; u32 len; u32 reserved; u32 reserved2 }` | `MAP` | 0 |
| `Vmar` | 3 | `PROTECT` | `{ u32 addr; u32 len; u32 prot; u32 reserved }` | `MAP` | 0 |
| `Vmar` | 4 | `ADVISE` | `{ u32 addr; u32 len; u32 advice; u32 reserved }` | `MAP` | 0 |
| `Vmar` | 5 | `SYNC` | `{ u32 addr; u32 len; u32 flags; u32 reserved }` | `MAP` | 0 |
| `Vmar` | 6 | `DESTROY` | — | `MAP` | 0 |
| `Vmo` | 1 | `READ` | `CellFileIo` | `READ` | bytes |
| `Vmo` | 2 | `WRITE` | `CellFileIo` | `WRITE` | bytes |
| `Vmo` | 3 | `CLONE` | `{ u32 flags; u32 reserved }` | `READ` | new `Vmo` |
| `Vmo` | 4 | `RESIZE` | `{ u64 size }` | `WRITE` | 0 |
| `Vmo` | 5 | `SYNC` | `{ u64 offset; u32 len; u32 flags }` | `WRITE` | 0 |
| `Vmo` | 6 | `INFO` | `{ u32 buf; u32 len }` | `READ` | `CellVmoInfo` |

`Vmar::MAP` returns the address in `$v1`; a `MAP_FIXED` hint outside the cell's own
allocated frames returns `E_NOTSUP` in v0.1 (open, §10).  `PROTECT` that adds
`PROT_EXEC` performs a data-cache writeback and instruction-cache invalidate over the
range; without that call, newly written code may not execute correctly.  `PROT_NONE` is
metadata only in SAS.  `Vmo::CLONE` with `CELL_VMO_COW` (1) is a copy-on-write request
that SAS cannot honour with hardware protection; the kernel returns `E_NOTSUP` in v0.1 and
an eager copy with `CELL_VMO_COPY` (0).

#### 4.6.6 `Socket` (type 7)

| No. | Method | Args | Rights | Result |
|---:|---|---|---|---|
| 1 | `BIND` | `{ u32 addr; u32 addr_len; u32 reserved; u32 reserved2 }` | `WRITE` | 0 |
| 2 | `CONNECT` | `{ u32 addr; u32 addr_len; u64 deadline }` | `WRITE` | 0 |
| 3 | `LISTEN` | `{ u32 backlog; u32 reserved }` | `INVOKE` | 0 |
| 4 | `ACCEPT` | `{ u64 deadline; u32 flags; u32 reserved }` | `READ` | new `Socket` handle |
| 5 | `SEND` | `CellSocketIo` | `WRITE` | bytes |
| 6 | `RECV` | `CellSocketIo` | `READ` | bytes |
| 7 | `SENDTO` | `CellSocketIo` + `addr` | `WRITE` | bytes |
| 8 | `RECVFROM` | `CellSocketIo` + out `addr` | `READ` | bytes |
| 9 | `SHUTDOWN` | `{ u32 how; u32 reserved }` | `WRITE` | 0 |
| 10 | `GETSOCKNAME` | `{ u32 addr; u32 addr_len; u32 reserved }` | `READ` | bytes |
| 11 | `GETPEERNAME` | the same | `READ` | bytes |
| 12 | `SETSOCKOPT` | `CellSockopt` | `WRITE` | 0 |
| 13 | `GETSOCKOPT` | `CellSockopt` | `READ` | bytes |
| 14 | `POLL` | `{ u64 port; u64 key; u32 events; u32 reserved }` | `READ` | revents |

The kernel never raises `SIGPIPE`; a send to a closed peer returns `E_PIPE`, and the libc
raises the signal unless `MSG_NOSIGNAL`.  `SCM_RIGHTS` is handle transfer on an `AF_UNIX`
socket; it uses the same `TRANSFER` rule as `Channel` (M9.5).

#### 4.6.7 `Channel` (type 2) and `Port` (type 3)

| Object | No. | Method | Args | Rights | Result |
|---|---:|---|---|---|---|
| `Channel` | 1 | `SEND` | `CellChannelMsg` | `WRITE` | 0 |
| `Channel` | 2 | `RECV` | `CellChannelMsg` | `READ` | bytes |
| `Channel` | 3 | `CALL` | `CellChannelCall` | `INVOKE` | bytes |
| `Channel` | 4 | `CLOSE` | — | — | 0 |
| `Channel` | 5 | `POLL` | `{ u64 port; u64 key; u32 events; u32 reserved }` | `READ` | revents |
| `Port` | 1 | `WAIT` | `CellPortWait` | `READ` | event count |
| `Port` | 2 | `POST` | `CellEvent` | `WRITE` | 0 |
| `Port` | 3 | `CLOSE` | — | — | 0 |

`Channel::CALL` sends the message and appends a one-shot reply `Channel` handle to the
message's handle array; the server sends exactly one reply on it.  If the server drops
the reply channel without replying, the caller gets `E_CANCELED`.  `SEND`/`RECV` are
one-way.  A message is at most 4096 bytes with up to 8 handles; bulk data is a `Vmo`
handle, not a copy (D12).

#### 4.6.8 `Device` (type 8), `Mmio` (9), `Irq` (10), `DmaWindow` (11)

| Object | No. | Method | Args | Rights | Result |
|---|---:|---|---|---|---|
| `Device` | 1 | `INFO` | `{ u32 buf; u32 len }` | `INVOKE` | `CellDeviceInfo` |
| `Device` | 2 | `MMIO_CLAIM` | `{ u32 index; u32 flags }` | `MANAGE` | `Mmio` handle |
| `Device` | 3 | `IRQ_CLAIM` | `{ u32 index; u32 flags }` | `MANAGE` | `Irq` handle |
| `Device` | 4 | `DMA_ALLOC` | `{ u32 size; u32 align; u32 flags; u32 reserved }` | `DMA` | `DmaWindow` handle |
| `Device` | 5 | `SOCKET` | `{ u32 domain; u32 type; u32 protocol; u32 flags }` | `INVOKE` | `Socket` handle |
| `Device` | 6 | `RESET` | `{ u32 flags; u32 reserved }` | `MANAGE` | 0 |
| `Device` | 7 | `POLL` | `{ u64 port; u64 key; u32 events; u32 reserved }` | `READ` | revents |
| `Mmio` | 1 | `MAP` | `{ u64 offset; u32 len; u32 cache; u32 addr_hint; u32 flags }` | `MAP` | `DEV` address |
| `Mmio` | 2 | `UNMAP` | `{ u32 addr; u32 len; u32 reserved; u32 reserved2 }` | `MAP` | 0 |
| `Mmio` | 3 | `INFO` | `{ u32 buf; u32 len }` | `READ` | range info |
| `Irq` | 1 | `BIND` | `{ u64 port; u64 key; u32 flags; u32 reserved }` | `INVOKE` | 0 |
| `Irq` | 2 | `ACK` | `{ u32 count; u32 reserved }` | `INVOKE` | 0 |
| `Irq` | 3 | `UNBIND` | — | `INVOKE` | 0 |
| `DmaWindow` | 1 | `MAP` | `{ u32 cache; u32 addr_hint; u32 flags; u32 reserved }` | `MAP` | address |
| `DmaWindow` | 2 | `SYNC` | `{ u32 offset; u32 len; u32 dir; u32 reserved }` | `DMA` | 0 |
| `DmaWindow` | 3 | `INFO` | `{ u32 buf; u32 len }` | `READ` | `CellDmaInfo` |
| `DmaWindow` | 4 | `FREE` | — | `DMA` | 0 |

`Device::SOCKET` requires a network-class `Device`; a cell without one gets `E_PERM`
from `socket()`.  `Mmio::MAP` returns an uncached `DEV` address.  `DmaWindow::MAP` with
`CELL_CACHE_NONE` (0) returns `DEV`; with `CELL_CACHE_WB` (1) it returns `MEM` and the
cell must call `SYNC` before and after DMA (D16).  `DmaWindow::INFO` returns the physical
address and length; only a cell with `DMA` sees it.

#### 4.6.9 `Thread` (type 13)

| No. | Method | Args | Rights | Result |
|---:|---|---|---|---|
| 1 | `JOIN` | `{ u64 deadline; u32 buf; u32 len }` | `READ` | exit status |
| 2 | `DETACH` | — | `MANAGE` | 0 |
| 3 | `CANCEL` | — | `MANAGE` | 0 |
| 4 | `EXIT` | `{ i32 status; u32 flags }` | `INVOKE` | does not return |
| 5 | `SIGNAL` | `{ i32 sig; u32 reserved }` | `INVOKE` | 0 (M9.5) |
| 6 | `SET_NAME` | `{ u32 buf; u32 len }` | `MANAGE` | 0 |
| 7 | `GET_INFO` | `{ u32 buf; u32 len }` | `READ` | `CellThreadInfo` |
| 8 | `SET_SIGALTSTACK` | `{ u32 base; u32 size; u32 flags; u32 reserved }` | `MANAGE` | 0 (M9.5) |
| 9 | `POLL` | `{ u64 port; u64 key; u32 events; u32 reserved }` | `READ` | revents |

Threads share the cell's address space and capability table.  `JOIN` returns the exit
status and reaps the `Thread` object; `DETACH` lets the kernel free it at exit.
`CANCEL` sets the deferred-cancellation flag; a blocking invocation returns
`E_CANCELED` at the next cancellation point.  `EXIT` is self-only: `$a0` must be the
calling thread, or the call returns `E_INVAL`.  `SET_SIGALTSTACK` is only meaningful once
synchronous faults are deliverable (M9.5).

### 4.7 Argument blocks for the core methods

All blocks are `repr(C)`, big-endian, naturally aligned; `u64` is 8-aligned.  A block is
an input unless a field is marked *out*; the kernel copies the whole block back.  The
`size` passed in `$a2` is the caller's block size; a method's minimum is the size of its
structure.

```c
/* Job::WAIT */
struct CellJobWaitIn {
    uint64_t child;      /* 0x00: 0 = any child */
    uint32_t flags;      /* 0x08: WNOHANG, WUNTRACED, WCONTINUED */
    uint32_t reserved;   /* 0x0C */
    uint64_t deadline;   /* 0x10 */
};                        /* 0x18 = 24 */

struct CellJobWaitOut {
    uint64_t child;      /* 0x00: out */
    uint32_t pid;        /* 0x08: out */
    int32_t  code;       /* 0x0C: out: exit code or fault code */
    uint32_t reason;     /* 0x10: out: CELL_EXIT_* */
    uint32_t reserved;   /* 0x14 */
};                        /* 0x18 = 24 */

/* Job::FORK; the same block is written in both copies (see §6.5) */
struct CellJobForkOut {
    uint64_t child;      /* 0x00: out: child Job, or the child's self Job */
    uint32_t pid;        /* 0x08: out: child pid in the parent, 0 in the child */
    uint32_t is_child;   /* 0x0C: out: 0 parent, 1 child */
    uint32_t status;     /* 0x10: out: 0 */
    uint32_t reserved;   /* 0x14 */
};                        /* 0x18 = 24 */

/* Job::SPAWN */
struct CellJobSpawn {
    uint64_t image;          /* 0x00: File with EXECUTE */
    uint64_t namespace_handle; /* 0x08: 0 = inherit */
    uint64_t cwd;            /* 0x10: File or 0 = inherit */
    uint32_t argv;           /* 0x18 */
    uint32_t argc;           /* 0x1C */
    uint32_t envp;           /* 0x20 */
    uint32_t envc;           /* 0x24 */
    uint32_t fds;            /* 0x28: CellFd[] input, parent handles */
    uint32_t fd_count;       /* 0x2C */
    uint32_t libc_data;      /* 0x30 */
    uint32_t libc_data_size; /* 0x34 */
    uint32_t flags;          /* 0x38: CELL_SPAWN_* */
    uint32_t reserved;       /* 0x3C */
    uint64_t deadline;       /* 0x40 */
    uint64_t child_out;      /* 0x48: out */
    uint32_t pid_out;        /* 0x50: out */
    uint32_t reserved2;      /* 0x54 */
};                            /* 0x58 = 88 */

/* Job::EXEC; the same fields as SPAWN without the child outputs */
struct CellJobExec {
    uint64_t image;          /* 0x00 */
    uint64_t namespace_handle; /* 0x08: 0 = inherit */
    uint64_t cwd;            /* 0x10: File or 0 = inherit */
    uint32_t argv;           /* 0x18 */
    uint32_t argc;           /* 0x1C */
    uint32_t envp;           /* 0x20 */
    uint32_t envc;           /* 0x24 */
    uint32_t fds;            /* 0x28 */
    uint32_t fd_count;       /* 0x2C */
    uint32_t libc_data;      /* 0x30 */
    uint32_t libc_data_size; /* 0x34 */
    uint32_t flags;          /* 0x38 */
    uint32_t reserved;       /* 0x3C */
    uint64_t deadline;       /* 0x40 */
};                            /* 0x48 = 72 */

/* File::READ / WRITE / VMO::READ / WRITE / READDIR */
struct CellFileIo {
    uint32_t buf;        /* 0x00: MEM pointer */
    uint32_t len;        /* 0x04 */
    uint64_t offset;     /* 0x08: explicit offset or the readdir cookie */
    uint32_t flags;      /* 0x10: CELL_IO_AT = 1 */
    uint32_t reserved;   /* 0x14 */
};                        /* 0x18 = 24 */

/* Namespace::OPEN */
struct CellNamespaceOpen {
    uint64_t dir;        /* 0x00: File or 0 = root */
    uint32_t path;       /* 0x08: MEM pointer to bytes */
    uint32_t path_len;   /* 0x0C: excluding NUL; <= 1023 */
    uint32_t flags;      /* 0x10: O_* subset */
    uint32_t mode;       /* 0x14 */
};                        /* 0x18 = 24 */

/* Namespace::STAT */
struct CellNamespaceStat {
    uint64_t dir;        /* 0x00 */
    uint32_t path;       /* 0x08 */
    uint32_t path_len;   /* 0x0C */
    uint32_t flags;      /* 0x10 */
    uint32_t buf;        /* 0x14: CellFileStat out */
    uint32_t buf_len;    /* 0x18 */
    uint32_t reserved;   /* 0x1C */
};                        /* 0x20 = 32 */

/* Namespace::RENAME / LINK */
struct CellNamespaceRename {
    uint64_t old_dir;    /* 0x00 */
    uint64_t new_dir;    /* 0x08 */
    uint32_t old_path;   /* 0x10 */
    uint32_t old_len;    /* 0x14 */
    uint32_t new_path;   /* 0x18 */
    uint32_t new_len;    /* 0x1C */
    uint32_t flags;      /* 0x20 */
    uint32_t reserved;   /* 0x24 */
};                        /* 0x28 = 40 */

/* Namespace::BIND */
struct CellNamespaceBind {
    uint64_t target;     /* 0x00 */
    uint32_t path;       /* 0x08 */
    uint32_t path_len;   /* 0x0C */
    uint64_t source;     /* 0x10 */
    uint32_t flags;      /* 0x18 */
    uint32_t reserved;   /* 0x1C */
};                        /* 0x20 = 32 */

/* File::SET_ATTR */
struct CellFileAttr {
    uint32_t mode;       /* 0x00 */
    uint32_t uid;        /* 0x04 */
    uint32_t gid;        /* 0x08 */
    uint32_t flags;      /* 0x0C: 1 mode, 2 uid, 4 gid, 8 atime, 16 mtime */
    int64_t  atime_sec;  /* 0x10 */
    uint32_t atime_nsec; /* 0x18 */
    uint32_t reserved0;  /* 0x1C */
    int64_t  mtime_sec;  /* 0x20 */
    uint32_t mtime_nsec; /* 0x28 */
    uint32_t reserved1;  /* 0x2C */
};                        /* 0x30 = 48 */

/* File::DEVCTL; the request payloads are device-class contracts */
struct CellFileDevctl {
    uint32_t request;    /* 0x00 */
    uint32_t in_buf;     /* 0x04 */
    uint32_t in_len;     /* 0x08 */
    uint32_t out_buf;    /* 0x0C */
    uint32_t out_len;    /* 0x10 */
    uint32_t reserved;   /* 0x14 */
};                        /* 0x18 = 24 */

/* Vmar::MAP */
struct CellVmarMap {
    uint64_t vmo;        /* 0x00 */
    uint64_t vmo_offset; /* 0x08 */
    uint32_t len;        /* 0x10 */
    uint32_t prot;       /* 0x14: PROT_* */
    uint32_t flags;      /* 0x18: MAP_* */
    uint32_t addr_hint;  /* 0x1C */
};                        /* 0x20 = 32 */

/* Socket::SEND / RECV / SENDTO / RECVFROM */
struct CellSocketIo {
    uint32_t buf;        /* 0x00 */
    uint32_t len;        /* 0x04 */
    uint32_t flags;      /* 0x08: MSG_* */
    uint32_t addr;       /* 0x0C: CellSockaddr pointer or 0 */
    uint64_t deadline;   /* 0x10 */
};                        /* 0x18 = 24 */

/* Socket::SETSOCKOPT / GETSOCKOPT */
struct CellSockopt {
    uint32_t level;      /* 0x00 */
    uint32_t option;     /* 0x04 */
    uint32_t buf;        /* 0x08 */
    uint32_t len;        /* 0x0C */
};                        /* 0x10 = 16 */

/* Channel::SEND / RECV; CALL uses the same block plus a reply handle */
struct CellChannelMsg {
    uint32_t size;         /* 0x00: data bytes, <= 4016 */
    uint32_t handle_count; /* 0x04: 0..8 (includes the reply handle for CALL) */
    uint32_t flags;        /* 0x08 */
    uint32_t reserved;     /* 0x0C */
    uint64_t handles[8];   /* 0x10 */
    uint8_t  data[4016];   /* 0x50 */
};                          /* 0x1000 = 4096 */

/* Channel::CALL: an input message and an output message */
struct CellChannelCall {
    uint32_t in_buf;     /* 0x00: CellChannelMsg in */
    uint32_t in_len;     /* 0x04 */
    uint32_t out_buf;    /* 0x08: CellChannelMsg out */
    uint32_t out_len;    /* 0x0C */
    uint64_t deadline;   /* 0x10 */
};                        /* 0x18 = 24 */

/* Port::WAIT */
struct CellPortWait {
    uint32_t events;     /* 0x00: CellEvent[] pointer */
    uint32_t max;        /* 0x04: 1..64 */
    uint64_t deadline;   /* 0x08 */
};                        /* 0x10 = 16 */

/* Job::THREAD_CREATE */
struct CellThreadCreate {
    uint32_t entry;        /* 0x00 */
    uint32_t arg;          /* 0x04 */
    uint32_t stack_base;   /* 0x08: 0 = allocate */
    uint32_t stack_size;   /* 0x0C: 0 = default (2 MiB) */
    uint32_t tls;          /* 0x10: TCB pointer, 0 = none */
    uint32_t flags;        /* 0x14: CELL_THREAD_DETACHED = 1 */
    uint32_t stack_base_out; /* 0x18: out */
    uint32_t stack_top_out;  /* 0x1C: out */
};                            /* 0x20 = 32 */
```

The optional blocks (`CellVmoInfo`, `CellDmaInfo`, `CellStatVfs`) are pinned by the
`native-abi` crate in M9; the method tables name their fields.  The blocks above are the
ones needed for the M9/M9.5 libc slice.

### 4.8 POSIX mapping

The native libc is the enforcement point (native-libc §4.9).  The table maps the calls
the M9/M9.5 profile needs; each row names the invocation and the authority it checks.

| POSIX | Invocation | Authority | Notes |
|---|---|---|---|
| `open`, `openat` | `Namespace::OPEN` | `INVOKE` + dir `READ\|EXECUTE` | returns `File`; libc allocates fd |
| `read`, `write` | `File::READ/WRITE` | `READ`/`WRITE` | offset in the `File` object |
| `pread`, `pwrite` | `File::READ/WRITE` + `CELL_IO_AT` | `READ`/`WRITE` | explicit offset |
| `lseek` | `File::SEEK` | `READ`/`WRITE` | `E_SPIPE` for pipes/sockets |
| `close` | `Job::CAP_DROP` | — | last fd reference drops the cap |
| `dup`, `F_DUPFD` | libc fd table | — | shares the `File` object and offset |
| `mmap` | `Job::VMO_CREATE`, `Vmar::MAP`, `File::GET_VMO` | `MANAGE`, `MAP`, `READ`/`MAP` | returns a `MEM` address |
| `munmap`, `mprotect` | `Vmar::UNMAP/PROTECT` | `MAP` | `PROT_NONE` is metadata only in SAS |
| `msync` | `Vmo::SYNC`/`File::SYNC` | `WRITE` | |
| `socket` | `Device::SOCKET` | `INVOKE` | no ambient network |
| `bind`, `connect`, `listen`, `accept` | `Socket::*` | `WRITE`/`INVOKE` | deadlines |
| `send`, `recv`, `sendto`, `recvfrom` | `Socket::*` | `READ`/`WRITE` | `MSG_NOSIGNAL` suppresses SIGPIPE |
| `poll`, `ppoll` | `File/Socket::POLL` + `Port::WAIT` | object `READ` + Port `READ` | readiness model, M9.5 |
| `fork` | `Job::FORK` | `MANAGE` | **deferred**; `E_NOTSUP` in v0.1 (§6.5) |
| `posix_spawn` | `Job::SPAWN` | `MANAGE` + image `EXECUTE` | libc builds the fd plan |
| `execve` | `Job::EXEC` | `MANAGE` + image `EXECUTE` | no return on success |
| `waitpid`, `waitid` | `Job::WAIT` | `READ` | libc encodes `wstatus` |
| `kill` | `Job::SIGNAL/KILL` | `INVOKE` | only Jobs the cell holds |
| `getpid`, `getppid` | `CellStart` / `Job::GET_INFO` | `READ` | |
| `clock_gettime` | `SYS_CLOCK_GETTIME` | — | absolute monotonic ns |
| `nanosleep` | `SYS_SLEEP` | — | |
| `getrandom`, `getentropy` | `SYS_GETRANDOM` | — | kernel CSPRNG |
| `pipe`, `pipe2` | `Job::PIPE_CREATE` | `MANAGE` | two `File` handles |
| `fcntl` | libc fd table + `File` flags | — | `F_SETFL` updates the `File` |
| `posix_devctl`, `ioctl` | `File::DEVCTL` | `INVOKE` | device-class payloads |
| `isatty` | `File::INFO` | `READ` | |
| `chdir`, `fchdir`, `getcwd` | `Namespace::OPEN` + libc cwd | dir `READ\|EXECUTE` | cwd string is libc state |
| `chmod`, `chown` | `File::SET_ATTR` | `MANAGE` | no uid 0 |
| `chroot` | `Namespace::CHROOT` | `MANAGE` | returns `EPERM` without it |
| `uname`, `sysconf` | `SYS_UNAME`, `SYS_SYSCONF` | — | |
| `pthread_*` | `Job::THREAD_CREATE`, `Thread::*`, `SYS_FUTEX_*` | `MANAGE`/`READ` | M9.5 for sync |
| `sched_yield` | `SYS_YIELD` | — | |

The libc maps a native status to `-1`/`errno` (or a pthread error number).  A capability
failure is `EPERM`, `EACCES` or `EBADF` as appropriate; it is never a silent success.  A
stale handle is `E_BADF` → `EBADF`; a revoked handle is `E_REVOKED` → `EBADF`.

---

## 5. Trap entry

### 5.1 Gate instruction and dispatch

The native gate is `syscall 0x1F7`:

```asm
    syscall 0x1F7        # native capability invocation
```

The MIPS `syscall` instruction is `SPECIAL` (opcode 0) with `funct = 0x0C` and a 20-bit
code field.  Code `0x1F7` encodes the instruction word `0x0000_7DCC`; IRIX 6.5 uses code
0, so the two are unambiguous.  The gate is a standalone instruction, not a delay-slot
instruction.

On a trap, the kernel:

1. Saves all 32 GPRs, `$hi`/`$lo`, `EPC`, `Cause`, `Status` and `BadVAddr` in the task's
   trap frame on its kernel stack.  `$k0`/`$k1` are the only scratch registers used by
   the entry sequence and are saved as part of the frame.
2. Reads the instruction word at `EPC` (masking `Cause.BD` if set) and extracts the
   20-bit code.
3. Dispatches on the current task's personality and the code:
   - native + `0x1F7` → `CellInvoke` (§5.2);
   - legacy + 0 → the IRIX syscall decoder (architecture §6.2);
   - native + anything else → terminate the cell with `BAD_TRAP`;
   - legacy + `0x1F7` → deliver `SIGSYS` to the legacy process.
4. Returns with `eret` to `EPC + 4` (or to the new entry for a successful `exec`).  The
   kernel never rewinds `EPC` to restart an invocation; the libc restarts at the API
   layer (§5.4).

`Cause.BD` must be clear for a native gate; a gate in a delay slot is `BAD_TRAP`.

### 5.2 Register convention

| Register | Meaning |
|---|---|
| `$v0` | Selector on entry; status on return |
| `$a0` | Handle (`u64`) |
| `$a1` | Block pointer (`u32`, zero-extended) |
| `$a2` | Block size (`u32`) |
| `$a3` | Block version (`u32`) |
| `$v1` | Result value (`u64`) on return |

The kernel preserves every other GPR, `$hi`/`$lo` and the FPU state (none in v0.1,
soft-float).  The caller may treat the gate as clobbering only `$v0`, `$v1` and `$at`.
A 32-bit argument with a non-zero upper 32 bits is `E_INVAL`.  The block pointer must be
8-aligned; the block size must be a multiple of 4 and at most 4096; a block size smaller
than the method's minimum is `E_INVAL`.

### 5.3 Return and error encoding

`$v0` is a 32-bit status:

| Value | Name | Typical libc mapping |
|---:|---|---|
| 0 | `CELL_OK` | success |
| 1 | `CELL_E_PERM` | `EPERM` |
| 2 | `CELL_E_NOENT` | `ENOENT` |
| 3 | `CELL_E_INTR` | `EINTR` |
| 4 | `CELL_E_IO` | `EIO` |
| 5 | `CELL_E_BADF` | `EBADF` |
| 6 | `CELL_E_AGAIN` | `EAGAIN`/`EWOULDBLOCK` |
| 7 | `CELL_E_NOMEM` | `ENOMEM` |
| 8 | `CELL_E_ACCESS` | `EACCES` |
| 9 | `CELL_E_FAULT` | `EFAULT` |
| 10 | `CELL_E_BUSY` | `EBUSY` |
| 11 | `CELL_E_EXIST` | `EEXIST` |
| 12 | `CELL_E_INVAL` | `EINVAL` |
| 13 | `CELL_E_NOSPC` | `ENOSPC` |
| 14 | `CELL_E_NOTDIR` | `ENOTDIR` |
| 15 | `CELL_E_ISDIR` | `EISDIR` |
| 16 | `CELL_E_NOTEMPTY` | `ENOTEMPTY` |
| 17 | `CELL_E_NOTSUP` | `ENOTSUP`/`EOPNOTSUPP` |
| 18 | `CELL_E_OVERFLOW` | `EOVERFLOW` |
| 19 | `CELL_E_TIMEDOUT` | `ETIMEDOUT` |
| 20 | `CELL_E_CANCELED` | `ECANCELED` |
| 21 | `CELL_E_REVOKED` | `EBADF` |
| 22 | `CELL_E_MSGSIZE` | `EMSGSIZE` |
| 23 | `CELL_E_PROTO` | `EPROTO` |
| 24 | `CELL_E_CONNREFUSED` | `ECONNREFUSED` |
| 25 | `CELL_E_NOTCONN` | `ENOTCONN` |
| 26 | `CELL_E_ADDRINUSE` | `EADDRINUSE` |
| 27 | `CELL_E_NETUNREACH` | `ENETUNREACH` |
| 28 | `CELL_E_HOSTUNREACH` | `EHOSTUNREACH` |
| 29 | `CELL_E_NAMETOOLONG` | `ENAMETOOLONG` |
| 30 | `CELL_E_LOOP` | `ELOOP` |
| 31 | `CELL_E_CHILD` | `ECHILD` |
| 32 | `CELL_E_SRCH` | `ESRCH` |
| 33 | `CELL_E_NOSYS` | `ENOSYS` |
| 34 | `CELL_E_PIPE` | `EPIPE` |
| 35 | `CELL_E_RANGE` | `ERANGE` |
| 36 | `CELL_E_DOM` | `EDOM` |
| 37 | `CELL_E_DEADLK` | `EDEADLK` |
| 38 | `CELL_E_NOLCK` | `ENOLCK` |
| 39 | `CELL_E_NOMSG` | `ENOMSG` |
| 40 | `CELL_E_IDRM` | `EIDRM` |
| 41 | `CELL_E_NOTTY` | `ENOTTY` |
| 42 | `CELL_E_SPIPE` | `ESPIPE` |
| 43 | `CELL_E_TXTBSY` | `ETXTBSY` |
| 44 | `CELL_E_FBIG` | `EFBIG` |
| 45 | `CELL_E_MLINK` | `EMLINK` |
| 46 | `CELL_E_ROFS` | `EROFS` |
| 47 | `CELL_E_NODEV` | `ENODEV` |
| 48 | `CELL_E_NXIO` | `ENXIO` |
| 49 | `CELL_E_NOTBLK` | `ENOTBLK` |
| 50 | `CELL_E_2BIG` | `E2BIG` |
| 51 | `CELL_E_MFILE` | `EMFILE` |
| 52 | `CELL_E_NFILE` | `ENFILE` |

`$v1` is zero on error and is never a negative errno.  The kernel does not know the
libc's `errno` numbering; the mapping table is generated from the `native-abi` crate and
lives in the libc.  Adding a status is additive; changing a value is a major ABI change.
`CELL_OK` is not a value that a status-returning method can confuse with data: the value
is in `$v1`.

Throughout this document, `E_FOO` in prose is shorthand for `CELL_E_FOO`; a libc `errno`
name is written without the extra underscore (`EACCES`, `ENOTSUP`).  The kernel never
returns a libc `errno`.

### 5.4 Deadlines, blocking, restart and cancellation

- A blocking method takes an absolute `deadline` in `CLOCK_MONOTONIC` nanoseconds.  `0`
  means infinite; a deadline in the past returns `E_TIMEDOUT` immediately.
- `E_AGAIN` means "would block" for a non-blocking call; `E_TIMEDOUT` means the deadline
  expired; `E_INTR` means a signal event is pending; `E_CANCELED` means the thread's
  deferred cancellation flag was set at a cancellation point.
- The kernel **never** restarts a call automatically.  The libc decides, using the POSIX
  `SA_RESTART` rule: if the signal is blocked or ignored, or `SA_RESTART` is set, the
  libc re-issues the same invocation with the remaining deadline.  Otherwise it returns
  `-1`/`EINTR`.
- A signal event posted on the cell's event `Port` makes any blocking invocation return
  `E_INTR` without consuming the event.  The libc's signal task consumes it.  A cell with
  no event `Port` is never interrupted by a signal.
- Cancellation is deferred.  `Thread::CANCEL` sets a flag; the next blocking invocation
  that is a cancellation point returns `E_CANCELED`.  Asynchronous cancellation and
  cancellation of a thread blocked in an invocation are optional; if not implemented,
  the libc returns `ENOTSUP` rather than hanging (native-libc §4.7).
- A `Job::KILL`/`STOP` does not wait for a deadline; it acts on the target immediately
  and wakes its waiters with `E_REVOKED`/`E_CANCELED`.

### 5.5 `copyin` and `copyout`

The kernel touches cell memory only through two helpers (names are illustrative):

```rust
fn copyin(cell: &Cell, ptr: u32, len: u32) -> Result<CellSlice<'_>>;
fn copyout(cell: &Cell, ptr: u32, src: &[u8]) -> Result<()>;
fn copyinstr(cell: &Cell, ptr: u32, max: u32) -> Result<&[u8]>;
```

`copyin`/`copyout` perform the §3.7 checks, hold a `VMAR` reference for the duration,
and use volatile/byte-wise copies or a validated slice.  They never panic on bad input;
a bad pointer is `E_FAULT`.  `copyinstr` stops at the first NUL, enforces `max` and
returns `E_NAMETOOLONG` when the string is not terminated.  A bulk I/O buffer is a
validated slice handed to the VFS for the duration of the call; the VFS must not retain
it.  A concurrent `munmap` of a range in use returns `E_BUSY`; a concurrent data race on
a buffer is the cell's bug, not a kernel fault.

The kernel never accepts a kernel pointer from a cell and never returns one.  The
`CellStart` and invocation blocks are copied; the kernel's internal `Cell`/`Job`/`VMAR`
structures are separate.

### 5.6 Difference from the IRIX 6.5 trap

| Property | Native cell | IRIX 6.5 process |
|---|---|---|
| Instruction | `syscall 0x1F7` | `syscall` (code 0) |
| Selector | `(type << 16) \| method` in `$v0` | IRIX syscall number in `$v0` |
| Arguments | `$a0` handle, `$a1` block, `$a2` size, `$a3` version | `$a0`–`$a3`, 5th–8th on the stack (o32); `$a0`–`$a5` and stack (n32) |
| Result | `$v0` status, `$v1` value | `$v0` result, `$a3` error flag |
| Error | `CELL_E_*` status | IRIX `errno` in `$v0`/`$a3` |
| Pointers | Cell `MEM`/`DEV` pointers, VMAR-validated | User kuseg pointers, `copyin`/`copyout` against the process's page table |
| Number space | New; no IRIX number is reused | IRIX 6.5 numbers |
| Personality | Kernel-mode SAS cell | User-mode MMU process with an ASID |
| Signals | Events on a `Port`; libc-owned numbering | Kernel-built IRIX signal frame |

The compatibility container's decoder is unchanged.  A native cell never executes an
IRIX syscall number, and a legacy process never sees a handle.  The kernel's object layer
is the only shared part.

### 5.7 Trap smoke test

The first M9 gate test is an assembly cell that:

1. issues `SYS_GETRANDOM` with a 16-byte buffer and checks `$v0 == 0` and `$v1 == 16`;
2. issues `SYS_CLOCK_GETTIME(CLOCK_MONOTONIC)` and checks the value is non-zero;
3. issues `Job::EXIT(0)` and never returns.

The second test is a Rust cell that opens a file through the namespace, reads it, writes
it to stdout, and exits.  Negative tests cover: a plain `syscall`, a bad gate code, a
forged handle, a stale generation, a pointer outside the VMAR, a block that overflows,
and a missing `EXECUTE` right.  Every negative test must terminate or return a status;
none may panic the kernel or hang.

---

## 6. Cell lifecycle, threads, TLS, signals and `fork`

### 6.1 Lifecycle

```text
created -> loaded -> running -> (exited | killed | faulted)
                             \-> zombie -> reaped
```

1. **Create.**  The parent calls `Job::SPAWN` (or `Job::EXEC` for itself).  The kernel
   creates a `Job`, allocates the capability table and the namespace, and asks the loader
   to map the image.
2. **Load.**  The loader validates the image and `CellManifest`, maps the VMARs, builds
   `CellStart` and the start page, and flushes the caches.
3. **Run.**  The main `Thread` enters at `_start`.  The job is `running`; it can create
   threads and child objects subject to its rights and quotas.
4. **Exit.**  `Job::EXIT(status, flags)` records the job's reason, wakes the parent's
   `WAIT` and posts a `CHILD` event; `Thread::EXIT` records a thread's status and wakes
   its joiners.  For a job exit, all threads stop; the image VMARs are freed; open objects
   are destroyed when their last capability drops.
5. **Reap.**  The parent calls `Job::WAIT` and consumes the status; the `Job` object is
   destroyed.  If the parent exits first, `init` (pid 1) adopts and reaps the child
   (open, §10).  A `Job` that is not reaped is a zombie and counts against the parent's
   child quota.

There is no unload, no hot-swap and no destructor callback into the cell.  A cell's
`atexit` handlers and `__attribute__((destructor))` functions run in the libc's exit
path, not in the kernel; if the cell is killed, they do not run.

`exec` is a lifecycle transition, not a new load: the image VMARs and threads are
replaced, the surviving fds are re-granted, and the new image starts at `_start` with a
fresh `CellStart` (§6.6).

### 6.2 Threads

A `Thread` is a scheduled task inside a cell.  Threads share the address space, the
capability table, the namespace, the fd table and the signal `Port`; they have their own
register state, stack, TCB and signal mask.

- `Job::THREAD_CREATE` takes an entry PC, an argument, a stack (or 0 to allocate), a TLS
  pointer and flags.  It returns a `Thread` handle and the actual stack bounds.  The
  kernel allocates the stack from the job's memory quota; the default is 2 MiB, the
  minimum 64 KiB.
- The new thread starts at `entry(arg)` with `$sp = stack_top`, `$ra = 0`, `$gp` equal to
  the image's `$gp`, `$28 = tls`, and interrupts enabled.  The libc's trampoline calls
  the user's start routine and then `Thread::EXIT`; it must not return.
- `Thread::JOIN(deadline)` blocks until the thread exits and returns its status;
  `Thread::DETACH` lets the kernel free the `Thread` object at exit.  A detached thread's
  handle is invalid after `DETACH`.
- `Thread::CANCEL` sets the deferred cancellation flag.  `Thread::SIGNAL` posts a signal
  event targeted at the thread (M9.5).
- The libc's `pthread_t` is a small `u32` id; the libc's thread table maps it to the
  `Thread` handle.  The kernel never sees a `pthread_t`.
- Scheduling classes are EDF for media and fair for the rest (D14).  `THREAD_CREATE`
  flags/policy for EDF and priority inheritance are M9.5; v0.1 uses fair scheduling for
  all threads.
- There is **no hardware guard page** in SAS.  The loader leaves a 4 KiB gap below the
  stack that is not part of any VMAR; the libc may place a canary.  A stack overflow is
  not trapped by the MMU.  This is a v0.1 limitation that per-cell address spaces fix.

### 6.3 Thread-local storage

v0.1 uses **pthread keys**, not ELF TLS.  The native target sets
`has-thread-local: false`, which selects `std`'s pthread-key implementation
(native-libc §4.4).

- **Thread pointer.**  `$28` is the thread pointer (TP), matching the MIPS TLS ABI
  register.  It points to a libc-owned TCB.  The kernel preserves `$28` across traps and
  sets it on `THREAD_CREATE`.
- **TCB.**  The libc owns the layout.  The ABI fixes only the first two words: `self`
  (a pointer to the TCB, for `pthread_self`) and `dtv` (0 in v0.1).  Key 0 is reserved
  for `errno`; keys 1..127 are `pthread_key_t` values.  `PTHREAD_KEYS_MAX` is at least
  128 and `PTHREAD_DESTRUCTOR_ITERATIONS` at least 4.
- **Main thread.**  `CellStart.tls_base`/`tls_size` are 0 in v0.1; the libc's `_start`
  allocates the main TCB on the stack, sets `$28`, and only then calls Rust.  This keeps
  the TCB layout out of the loader.
- **New threads.**  The libc allocates the TCB and passes its address to
  `THREAD_CREATE`; the kernel sets `$28` before entering the trampoline.
- **ELF TLS.**  `PT_TLS`, `__tls_get_addr`, the static TLS block and the DTV are post-v1
  (open, §10).  When they arrive, `$28` and the first two TCB words stay the same; the
  DTV word becomes valid and `has-thread-local` may become `true`.  The pthread-key ABI
  remains for source compatibility.
- **`errno`.**  `__errno_location` returns a per-thread pointer; key 0 (or a reserved
  TCB slot) holds it.  `errno` starts at zero in each new thread and is not cleared by a
  successful call.

### 6.4 Signals and events

Signals are **events**.  The kernel does not know the libc's signal numbers; it carries
an opaque 32-bit `sig` and a reason.  The libc's signal table maps it to `SIG*`,
`siginfo_t` and the handler.

**Event record.**  A `Port` delivers 32-byte events:

```c
struct CellEvent {
    uint32_t kind;    /* 0x00: CELL_EVENT_* */
    uint32_t flags;   /* 0x04 */
    uint64_t key;     /* 0x08: registration key or target thread */
    uint64_t value;   /* 0x10: signal number, pid, count, ... */
    uint64_t aux;     /* 0x18: si_code, status, fault address, ... */
};                     /* 0x20 = 32 */
```

| Kind | Meaning | `value` | `aux` |
|---:|---|---|---|
| 1 | `SIGNAL` | libc signal number | target thread id, `si_code` |
| 2 | `CHILD` | child pid | exit reason/status |
| 3 | `READY` | registration key | revents |
| 4 | `IRQ` | IRQ key | count |
| 5 | `CHANNEL` | channel key | message available |
| 6 | `TIMER` | timer key | expiration count |
| 7 | `EXIT` | thread id | exit status |
| 8 | `REVOKED` | handle index | reason |

Where a row names two values for one 64-bit field, the kernel packs them as
`(hi << 32) | lo`: for `SIGNAL`, `aux = (target_thread << 32) | si_code`; for `CHILD`,
`aux = (reason << 32) | status`.  The libc unpacks them.

**Delivery model.**  v0.1 delivers **asynchronous signals at safe points**, not by
interrupting an arbitrary instruction.  The kernel posts a `SIGNAL` event to the cell's
event `Port` and makes blocking invocations return `E_INTR`.  The libc's signal task
(the thread blocked on the `Port`) consumes the event, checks the target thread's signal
mask, and arranges delivery at the next safe point: a syscall return, a cancellation
point, `sigwait`, or an explicit `pthread_testcancel`.  The handler runs on the target
thread with a libc-built `ucontext_t`.  A thread in a tight user-space loop may not see
the signal promptly.  This is a deviation from POSIX asynchronous delivery and is open
(§10); M9.5 may add kernel-assisted delivery at the next timer tick.

**Uncatchable signals.**  `Job::KILL` and `Job::STOP` are the kernel operations behind
`SIGKILL` and `SIGSTOP`; they act immediately and cannot be blocked or caught.
`SIGCONT` is `Job::CONT`.  The libc maps the signal numbers to these methods and does not
send them as events.

**`SIGCHLD`.**  The kernel posts a `CHILD` event when a child exits, stops or continues.
`Job::WAIT` is the authoritative reaper; the event is a hint.  A lost event does not
lose the status.

**`SIGPIPE`.**  The kernel never raises it.  A write to a closed pipe/socket returns
`E_PIPE`; the libc raises `SIGPIPE` unless `MSG_NOSIGNAL` or the signal is ignored.

**`SIGSEGV`, `SIGBUS`, `SIGILL`, `SIGFPE`.**  In v0.1, a synchronous fault terminates
the cell with reason `FAULTED`; the parent sees a signal-like status.  Catching a
synchronous fault requires a kernel-built `ucontext_t` on the alternate stack, which is
M9.5 work.  `sigaltstack` stores the libc state but has no effect until then.

**Masks and dispositions.**  `sigprocmask`, `pthread_sigmask`, `sigaction` and
`sigaltstack` are libc state.  The kernel does not block signals; a blocked signal stays
pending on the `Port` until the libc unmasks it.  The libc's signal task is the only
consumer.

**Authority.**  `kill(pid, sig)` works only for `Job`s the cell holds a capability for:
its own, its children, or a `Job` granted by the session manager.  There is no ambient
"same uid" rule.  `init` and the session manager hold the caps for the processes they
manage and implement the Unix-shaped `kill` policy above the ABI.

### 6.5 `fork`

The ABI defines `Job::FORK` and `CellJobForkOut` for a future implementation, but
**general `fork` is not implementable in the v0.1 kseg0 SAS model**:

- A `fork` must give the child the same virtual addresses as the parent, because the
  child resumes at the same PC with the same stack and pointers.
- In kseg0, virtual address equals physical address.  A copy of the parent's memory in
  new physical frames has different virtual addresses.  Relocating the child would
  require knowing which words are pointers, which is impossible for arbitrary Rust code.
- Copy-on-write needs a write-protection fault to trigger the copy.  kseg0 has no page
  protection and no TLB entry per cell, so COW cannot be implemented.
- A "swap-based fork" (keep one physical copy per cell and swap the active cell's memory
  on every context switch) preserves addresses and is general, but it is O(memory) per
  switch and unacceptable for a scheduler.
- A "spawn plus state transfer" fork (serialise the heap, stack and registers into a new
  cell) is a checkpoint/restore mechanism, not POSIX `fork`, and cannot handle arbitrary
  pointers.

Therefore:

| Milestone | `fork` status |
|---|---|
| M9 | `Job::FORK` returns `E_NOTSUP`; `fork`/`_Fork` return `-1`/`ENOSYS`; `posix_spawn`/`exec` are the process creation path |
| M9.5 | `std::process::Command` uses `posix_spawn`; `fork` remains unsupported |
| Post-M9.5 | A per-cell mapped address space (kseg2 or kuseg) enables COW `fork`; the `CellJobForkOut` contract is ready |

The contract, for that future implementation:

- `Job::FORK` duplicates the calling thread and the cell's VMARs (COW or copy), the
  capability table, the fd table and the namespace.  Other threads are not duplicated.
- In the parent, the out block has `is_child = 0`, `pid = child pid` and
  `child = child Job`; in the child, `is_child = 1`, `pid = 0` and `child = self Job`.
  Both copies return from the trap; the libc's `fork()` wrapper reads `is_child`.
- The child's slot 1 is retargeted to the child `Job` with the same index and generation,
  so the cached `CELL_SELF_HANDLE` remains valid.  Slot 2 is shared (or retargeted with
  the same generation).  The kernel rewrites the child's `CellStart.pid`/`ppid`.
- Capabilities to other `Thread` objects are revoked in the child; the libc's `fork()`
  wrapper resets its thread table to the calling thread and runs `pthread_atfork`
  handlers in the POSIX order.
- `FD_CLOFORK` fds are closed by the child's libc before `fork()` returns.
- `MAP_SHARED` VMOs remain shared; `MAP_PRIVATE` mappings are copied (eagerly in SAS,
  COW in a mapped cell).

Until then, a native programme that calls `fork` gets a documented `ENOSYS`.  This is a
deliberate product decision: `posix_spawn` is the preferred native path (architecture
§5.4), and `std::process::Command` uses it.  The M9 milestone's "`fork`/`exec` on a small
cell works" should be amended to "`posix_spawn`/`exec` works; `fork` returns `ENOSYS`"
(§11).

### 6.6 `posix_spawn` and `exec`

`posix_spawn` is the native process creation path.  The libc, not the kernel, applies
`posix_spawn_file_actions`:

1. Start from the parent's fd table.
2. Apply `addopen`, `addclose`, `adddup2`, `addchdir` and `addfchdir` in order,
   resolving paths in the parent's namespace and duplicating handles as needed.
3. Build a `CellFd[]` array of the resulting handles and fd flags.
4. Call `Job::SPAWN` with the image `File`, `argv`/`envp`, the `CellFd[]` array, the
   child's cwd and namespace handles, and the libc start blob.
5. On success, the kernel returns the child `Job` handle and pid; the libc records them
   in its child table.

The kernel:

1. Validates `MANAGE` on the parent `Job` and `EXECUTE` on the image `File`.
2. Creates a child `Job` and capability table (slots 1 and 2 reserved, generation 1).
3. Loads the image, builds the child's `CellStart`, rewrites the `CellFd[]` array with
   the child's handles, and installs the capabilities.
4. Starts the main thread at `_start` and returns the child handle/pid to the parent.

The child's fd table is built by the child's libc from `CellStart.fds`, not by the
kernel.  The kernel only guarantees the handles are valid and the fd numbers/flags are
as requested.

`Job::EXEC` is the same load in place of the current cell:

- The image, stack, start page, VMARs and (in v0.1) all other threads are replaced.  A
  multi-threaded cell's `exec` returns `E_BUSY` in v0.1; the libc reports `ENOTSUP` or
  requires the caller to be single-threaded.  This avoids tearing down a running thread
  set under the caller.
- The capability table is rebuilt: slot 1 self `Job`, slot 2 namespace, then the
  surviving fd handles.  Handles not in the fd table are dropped, so `exec` attenuates
  capabilities rather than amplifying them.
- The fd numbers are preserved; the handles may be new slots.  The open file
  descriptions (and their offsets) are shared.
- Signal dispositions reset to default except ignored signals; the signal mask is
  preserved unless the caller passes a new one; the libc start blob carries the details.
- On success `Job::EXEC` does not return; the new `_start` runs.  On failure it returns
  an error and the old image continues.

`posix_spawn` attributes (`setsid`, `setpgid`, `setpgroup`, `setsigmask`, `setsigdef`,
`sched*`) are split: the `Job` attributes (`setsid`, `setpgid`, `setpgroup`) are in the
spawn block flags and are applied by the kernel; the signal mask/default and scheduler
attributes are in the libc start blob and are applied by the child's libc.  The exact
subset is M9.5.

`CellJobSpawn.flags` and `CellJobExec.flags` use the same bits:

| Bit | Flag | Meaning |
|---:|---|---|
| 0 | `CELL_SPAWN_SETSID` | new session |
| 1 | `CELL_SPAWN_SETPGID` | set the process group to `pgid` |
| 2 | `CELL_SPAWN_SETPGROUP` | set the process group to the child pid |
| 3 | `CELL_SPAWN_SETSIGMASK` | apply the libc blob's signal mask |
| 4 | `CELL_SPAWN_SETSIGDEF` | apply the libc blob's default dispositions |
| 5 | `CELL_SPAWN_SETSCHED` | apply the libc blob's scheduler attributes |

Every inherited fd handle must carry `TRANSFER`; otherwise `SPAWN` returns
`CELL_E_PERM` before creating the child.  Bits 6–31 are reserved and must be zero.

### 6.7 Panic at the cell boundary

A native cell is a Rust programme; a panic must terminate the cell, not the kernel.

**Phase 1 (`panic = "abort"`, M9).**  The SDK's panic handler writes a diagnostic to
fd 2 with a raw `File::WRITE` invocation (no allocation, no locks), then calls
`Job::EXIT(101, CELL_EXIT_PANIC)`.  The kernel records the panic reason and wakes the
parent.  The handler must not unwind; there is no unwind information for the kernel's
trap frame.

**Phase 2 (`panic = "unwind"`, M9.5).**  The SDK's `entry!` wraps `main` in
`catch_unwind`.  A panic that escapes `main` is caught, printed and turned into
`Job::EXIT(101, CELL_EXIT_PANIC)`.  A panic in a spawned thread is caught by the thread
trampoline; the thread exits with the payload and `join` returns `Err`, while the cell
continues (the `std::thread` behaviour).  `_Unwind_*` must be provided by the native
runtime or the `unwinding` crate with MIPS support (research report §2.4); `panic =
"abort"` alone does not remove the dependency.

**Never across the gate.**  Unwinding must never cross the native gate or a signal
frame.  The kernel's trap frame has no unwind tables; a panic in a signal handler or in
a trap wrapper is not supported in v0.1.  A panic in a destructor during cell exit is
also not recoverable; the cell exits with a panic reason.

The parent sees `reason = PANICKED` and a status of 101; `waitpid` reports a normal exit
with status 101, matching Rust's default.  A core dump is not produced in v0.1 (open).

### 6.8 Exit and wait

`Job::EXIT(status, flags)` terminates the calling cell; `Thread::EXIT(status, flags)`
terminates one thread.  Exit reasons:

| Reason | Meaning | libc `wstatus` |
|---:|---|---|
| 0 | `EXITED` | `WIFEXITED`, code in bits 8–15 |
| 1 | `KILLED` | `WIFSIGNALED`, SIGKILL |
| 2 | `FAULTED` | `WIFSIGNALED`, SIGSEGV/SIGBUS from `code` |
| 3 | `PANICKED` | `WIFEXITED`, code 101 (or `WIFSIGNALED` with SIGABRT) |
| 4 | `STOPPED` | `WIFSTOPPED` (M9.5) |
| 5 | `CONTINUED` | `WIFCONTINUED` (M9.5) |

`Job::WAIT` takes a child handle filter (0 = any), flags (`WNOHANG`, `WUNTRACED`,
`WCONTINUED`) and a deadline.  It returns the child `Job` handle, pid, code and reason.
One wait consumes the status; a second wait for the same child returns `E_CHILD`.  A
`WNOHANG` wait with no ready child returns `E_AGAIN`; the libc returns 0.

A zombie `Job` holds its exit status until it is reaped or its last capability drops.  If
the parent exits first, `init` adopts the child (open: reparent versus kill; the
recommended default is reparent, POSIX-like).  The adopted child's `Job` handle is
installed in `init`'s table with `MANAGE|READ|INVOKE`.

---

## 7. Namespace and file descriptors

### 7.1 The `Namespace` object

A `Namespace` is a capability-held mount/bind tree:

- a root `File` capability (a directory);
- an ordered mount table of `(mountpoint, File)` pairs;
- a private/global flag and a reference to the owning `Job` for accounting;
- a namespace id used to reject cross-namespace directory handles.

A cell always has a `Namespace` handle in slot 2, but the namespace may be **empty** (no
root).  An empty namespace denies every path operation with `CELL_E_ACCESS`; the cell can still
use the handles it was given.  This preserves "a cell gets a namespace" without inventing
an ambient root.

The **global namespace** is a `Namespace` object that `init` creates and grants to most
cells.  It is a capability like any other; a path in it does not grant authority beyond
the rights of the namespace's root and the cell's handles.  A private namespace is built
with `Job::NAMESPACE_CREATE` or `Namespace::CHROOT`; `bind`/`mount` compose it.

### 7.2 Path resolution

- A path is a byte string, not necessarily UTF-8; `PATH_MAX` is 1024 including the NUL,
  `NAME_MAX` is 255.
- A relative path starts at the supplied directory `File` (or the namespace root when
  `dir == 0`).  An absolute path starts at the namespace root, **not** at a global `/`.
- `..` at the namespace root stays at the root.  A directory handle from a different
  namespace is rejected (`E_INVAL`/`CELL_E_ACCESS`), so a cell cannot walk out of its tree.
- Symbolic links are resolved inside the namespace; an absolute symlink target is
  relative to the namespace root.  `SYMLOOP_MAX` is 8; exceeding it is `E_LOOP`.
- Crossing a mount point follows the mount table; `..` at a mount root returns to the
  mountpoint's parent, not the source's parent.
- `O_NOFOLLOW` returns a `File` for the symlink itself; `READLINK` reads it.
- A path never grants authority.  The resulting `File` capability is derived from the
  namespace's root rights and attenuated by the open flags; if the root lacks `WRITE`,
  an `O_WRONLY` open fails with `CELL_E_ACCESS` even if the mode bits would allow it.
- Mode bits are checked **below** the capability ceiling, using the uid/gid labels.
  `MANAGE` on the file or directory bypasses the mode check.  There is no uid 0.

### 7.3 `*at` semantics

- `openat`, `fstatat`, `unlinkat`, `mkdirat`, `renameat`, `linkat`, `symlinkat`,
  `readlinkat`, `fchmodat`, `fchownat`, `faccessat`, `utimensat` and `futimens` take a
  `dirfd`.  The libc resolves it to a `File` handle and passes that as the `dir` field.
- `AT_FDCWD` is a libc pseudo-fd: it means "use the cell's current-directory `File`
  handle".  The kernel never sees `AT_FDCWD`.
- `AT_SYMLINK_NOFOLLOW`, `AT_REMOVEDIR`, `AT_SYMLINK_FOLLOW` and `AT_EACCESS` are passed
  through as native flags.  `AT_EMPTY_PATH` is optional/`E_NOTSUP` in v0.1.
- `renameat` and `linkat` take two directory handles; both must be in the same namespace.
- A directory fd carries `READ|EXECUTE` (read and search).  A `File` opened with
  `O_PATH` is not part of the v1 profile; a directory handle is the substitute.

### 7.4 The file-descriptor table

The fd table lives in the **libc**, not the kernel.  The kernel has no native fd table;
a native cell's fds are a libc fiction over handles.  This keeps the kernel small and
makes `dup`, `F_DUPFD` and `CLOEXEC` libc operations.

An fd table entry is:

```c
struct FdEntry {
    uint64_t handle;      /* the capability */
    uint32_t refcount;    /* shared by dup'd fds */
    uint32_t flags;       /* FD_CLOEXEC, FD_CLOFORK */
};
```

The **open file description** lives in the kernel `File` object: its offset, `O_APPEND`
and `O_NONBLOCK` flags.  `dup` copies the handle reference, not the offset, so the two fds
share the offset as POSIX requires.  When `fork` exists, it copies the fd table and the
capability table and the `File` objects (and offsets) are shared.  `posix_spawn` passes
the handles; the child shares the open file descriptions.  `exec` preserves the
non-`CLOEXEC` fds and their offsets.

- fds 0, 1 and 2 are pre-populated from the `stdin`/`stdout`/`stderr` caps.  If a
  standard fd is absent, the libc opens `/dev/null` for it at startup (POSIX/`std`
  behaviour) or leaves it closed if the namespace denies it.
- `open` allocates the lowest free fd.  `dup` returns the lowest free fd >= the
  requested minimum; `dup2` closes the target first; `dup3` sets `O_CLOEXEC`.
- `close` decrements the reference; the last close calls `Job::CAP_DROP`.  A close of an
  fd that is in use by another thread is the cell's race.
- `F_GETFD`/`F_SETFD` read/write `FD_CLOEXEC`/`FD_CLOFORK`; `F_GETFL`/`F_SETFL` read/write
  the `File` flags; `F_DUPFD`/`F_DUPFD_CLOEXEC` duplicate.
- `OPEN_MAX` is 1024; the default per-cell limit is 256 and `setrlimit` may raise it.
  `FD_SETSIZE` is 1024.  A handle shortage is `E_NOMEM`/`EMFILE`; the cap table default of
  256 is enough because fds share handles.
- `FromRawFd` is `unsafe` in Rust; the libc still validates the fd, the handle generation
  and the flags on every use.  A stale or forged fd is `EBADF`; it is never dereferenced.
- `SCM_RIGHTS` transfers a handle over an `AF_UNIX` socket or a `Channel`; the sender
  needs `TRANSFER`, the receiver gets a new slot, and `MSG_CMSG_CLOEXEC` sets the fd flag
  in the receiver.  The transfer is atomic (all handles or none).

### 7.5 `posix_spawn_file_actions`

The libc executes the actions in the parent, before `Job::SPAWN`:

| Action | Libc implementation |
|---|---|
| `addopen(fd, path, flags, mode)` | open in the parent's namespace; place at `fd` |
| `addclose(fd)` | remove `fd` from the child plan |
| `adddup2(fd, newfd)` | duplicate the handle; place at `newfd` |
| `addchdir(path)` | resolve to a `File`; pass as `cwd` |
| `addfchdir(fd)` | use the fd's handle as `cwd` |

The kernel receives only the resulting `CellFd[]`, `cwd` and `namespace_handle`; it never
resolves a path for `spawn`.  This keeps path policy in one place (the namespace) and
avoids a second resolver in the kernel.  The child's libc builds its fd table from the
rewritten `CellFd[]`; the fd numbers and `CLOEXEC`/`CLOFORK` flags are preserved.

`FD_CLOEXEC` fds are excluded from the `exec` and `spawn` arrays.  `FD_CLOFORK` fds are
included for `spawn` but closed by the child's libc after a future `fork`; since `fork`
is deferred, the flag is stored and has no effect in v0.1.

### 7.6 No ambient root

The native world has **no ambient root**:

- no global `/`; a path is resolved in a `Namespace` capability;
- no uid 0; uid/gid are labels and confer no authority;
- no implicit device or network access; `Device`/`Mmio`/`Irq`/`DmaWindow` caps are
  granted explicitly;
- no setuid or privilege gain on `exec`; `AT_SECURE` is always 0;
- `chroot` requires `MANAGE` on the namespace and returns `EPERM` otherwise;
- `kill` reaches only `Job`s the cell holds;
- `chown`/`chmod`/`setuid`/`setgid`/`setgroups` require the matching `MANAGE` right and
  otherwise return `EPERM`;
- a `File` handle from another namespace cannot be used as a directory handle.

A name is an API, never an authority grant (D4, D11).

### 7.7 How the libc and `std` consume it

The libc is the enforcement point (native-libc §4.9, research report §5.4):

- `std::fs` calls the libc's `*at` family; the libc resolves the namespace and checks
  the `File` rights.
- `std::net` calls `socket`; the libc needs a network `Device` cap (named `net` in the
  manifest) or a pre-opened `Socket`.  Without it, `socket()` returns `EACCES`.
- `std::process::Command` uses `posix_spawn` (the target patch adds `irix7` to the
  `posix_spawn` list); the libc applies the file actions and passes the fd plan.
- `std::thread` uses `pthread_create` over `THREAD_CREATE`; `std::sync` uses futexes.
- `std::env::current_exe` uses `getexecname`, backed by `AT_EXECFN`.
- `std::fs::File::lock` uses `fcntl` record locks or `lockf`; `flock` is not provided and
  must not fake success.
- `std::os::unix` is re-exported by the target patch; no raw handle is exposed to safe
  Rust.  `FromRawFd` is `unsafe` and the libc validates the fd.

---

## 8. Versioning, limits and security

### 8.1 ABI version negotiation

The ABI version is `(abi_major, abi_minor)`; this document is `0.1`.  The cell image's
`CellManifest` records the version it was built for; the loader copies it into
`CellStart`.

- The kernel accepts `abi_major == 0` and `abi_minor <= 1` in v0.1.  A cell built for a
  newer major is refused at load; a cell built for a newer minor is refused if the kernel
  does not know the fields it needs.
- The libc checks the same fields against its own minimum.  A libc that needs a field
  added in minor 2 refuses a minor 1 kernel and exits with a diagnostic.
- Invocation blocks have their own `version` (1 in v0.1) and `size`.  A method may accept
  a range of block versions; an unknown version is `E_NOTSUP`, not a silent reinterpret.
- Method numbers and object types are never reused.  A removed method returns
  `E_NOTSUP` for one major version and is then retired; its number stays reserved.
- The ABI becomes 1.0 only after the M9 conformance tests and the `native-abi` layout
  assertions pass.  Until then, any change is allowed with a version bump.
- The compatibility container has its own IRIX 6.5 versioning and is unaffected by this
  policy (architecture §5.7, native-libc §8).

### 8.2 Size and offset rules

- All ABI structures are `repr(C)`, big-endian, naturally aligned, free of bitfields,
  `bool` and implicit enum discriminants.  Enums are `u32` constants with validation.
- A field is added only at the end of a structure or in its reserved area; existing
  offsets never move.  A structure grows only when the reserved area is exhausted.
- `size` is part of the structure; a reader checks `size` before reading a field.  A
  writer zeroes what it does not set.
- Reserved fields are zero on write; a reader rejects a non-zero reserved field for the
  version it knows.  Unknown flag bits are rejected, not ignored.
- Every structure has compile-time `size_of`, `align_of` and `offset_of!` assertions in
  the `native-abi` crate, and a C oracle that prints the same values.  A mismatch fails
  the build or the layout test.
- A C header (`native-abi.h`) is generated from the crate or checked against it; there is
  no hand-maintained second copy.
- All pointers are 32-bit; all counts are bounded before use; all multiplications are
  checked.

### 8.3 Quotas and limits

| Resource | Default | Hard cap | Exceeded by |
|---|---:|---:|---|
| Capability slots per cell | 256 | 4096 | `E_NOMEM` from `CAP_DUP`/creation |
| VMARs per cell | 256 | 1024 | `E_NOMEM` |
| VMOs per cell | 256 | 1024 | `E_NOMEM` |
| Threads per cell | 32 | 64 | `E_AGAIN` |
| Child jobs per cell | 32 | 64 | `E_AGAIN` |
| Channels per cell | 64 | 256 | `E_NOMEM` |
| Ports per cell | 8 | 32 | `E_NOMEM` |
| Open fds (libc) | 256 | 1024 | `EMFILE` |
| Memory per cell | 32 MiB | 128 MiB | `E_NOMEM` |
| Stack per thread | 2 MiB | 16 MiB | `E_NOMEM` |
| Invocation block | 4096 B | 4096 B | `E_INVAL` |
| I/O per call | 1 MiB | 1 MiB | `E_INVAL` |
| Handles per message | 8 | 8 | `E_MSGSIZE` |
| Path | 1024 B | 1024 B | `E_NAMETOOLONG` |
| Name | 255 B | 255 B | `E_NAMETOOLONG` |
| `argv`+`envp` | 128 KiB | 128 KiB | `E_2BIG` |
| Events per `Port::WAIT` | 16 | 64 | `E_INVAL` |

The defaults are recommendations, not frozen values.  The kernel enforces them per
`Job`; a child inherits the parent's quota policy and may be given less.  A cell can read
its usage with `Job::GET_INFO(USAGE)`.  `setrlimit`/`getrlimit` map the subset that makes
sense; `RLIMIT_CORE` is 0, `RLIMIT_NOFILE` is the fd limit, `RLIMIT_NPROC` the thread
limit, `RLIMIT_AS` the memory quota.  CPU is not capped (D14: no admission control in
v1).

### 8.4 Security rules

1. **Validate every handle** on every use: index, generation, type, rights.  A stale or
   forged handle is `E_BADF`; it is never dereferenced.
2. **Rights attenuate only.**  No operation creates a capability with rights the caller
   did not hold.  `CAP_DERIVE` checks the subset; `Channel` transfer checks `TRANSFER`.
3. **Revocation is recursive and generation-checked.**  A revoked capability and all its
   descendants are invalid in every cell; waiters wake with `E_REVOKED`.
4. **No ambient authority.**  No global root, no uid 0, no implicit device or network,
   no setuid, no pid-based `kill`.
5. **No raw kernel callbacks.**  The kernel never calls a cell-supplied function pointer
   after the initial entry.  Threads start at a registered entry; events go through
   `Port`s.  The only "callbacks" are the initial `_start` and the thread trampoline,
   which are task entries, not registrations.
6. **No cell-writable kernel state.**  `CellStart`, the cap manifest and the fd array are
   read-only metadata; the kernel's `Cell`/`Job`/`VMAR` structures are separate.  There is
   no shared control block that bypasses the table.
7. **No kernel pointers in cell structures.**  All cell-visible pointers are `MEM`/`DEV`
   addresses validated by §3.7.
8. **`copyin`/`copyout` everywhere.**  The kernel never trusts a cell pointer, even from
   a "safe" cell; the check protects against a buggy libc or `unsafe` driver.
9. **Quotas per cell.**  One cell cannot exhaust the capability table, frame allocator,
   ports or scheduler queues of the system.
10. **`EXECUTE` is kernel-code authority in v0.1.**  A cell with `EXECUTE` on an image
    can spawn it as a kernel-mode SAS cell.  The capability must be treated as highly
    privileged; only trusted, verified images should be grantable until mapped cells
    exist.  The boot bundle's digests (D26) are the integrity gate; user-installed
    native binaries are not isolated in v0.1.
11. **The MCP cell is full root** (D30).  It uses the same ABI as any cell; there is no
    private syscall.  The bearer token is the gate; there is no audit in v0.1; the
    watchdog is the recovery path.
12. **The kernel shell is a debug path.**  It is not a cell and not part of this ABI; it
    can inspect and reset the machine.  It must not be reachable from a cell without a
    capability.
13. **Safe Rust is the memory-safety boundary for normal cells; `unsafe` driver code is
    trusted.**  A bug in a driver can corrupt memory; the review and the driver
    interface spec are the mitigation.
14. **SAS is not isolation.**  A cell that contains `unsafe` can read/write other cells
    and devices.  Do not run untrusted native binaries until per-cell address spaces
    and/or user mode exist.

Threat model:

| Adversary | Capability model | Memory safety |
|---|---|---|
| Safe Rust cell with a bug | Holds only its caps; cannot exceed them | Rust prevents memory unsafety |
| Driver cell with documented `unsafe` | Holds only its device caps | Trusted; a bug can corrupt memory |
| Malicious cell with `unsafe` | Can exceed caps by raw access | No hardware isolation in SAS |
| Legacy IRIX binary | Compat translation; no handles | MMU/kuseg/ASID isolation |
| MCP cell | Full root | Trusted; a bug is a kernel bug |

### 8.5 Revocation and audit

Revocation is the security backstop: a parent can revoke a delegated device, file or
job capability and know that every copy and derivation is invalid.  The generation check
makes the revocation immediate for subsequent invocations; a blocked invocation wakes
with `E_REVOKED`.  A driver that caches a device capability must re-check it on every
invocation; the kernel does.

There is **no audit trail** in v0.1.  Capability invocations are not logged; the MCP has
full root with a bearer token and no audit (D30).  A future audit object (a `Port` of
invocation events or a ring buffer readable by a privileged cell) is an open item (§10).
The kernel console's panic/trace output is a debug aid, not an audit log.

### 8.6 MCP boundary

The MCP control plane is a native cell (or, in M3, a kernel module exposing the same
JSON-RPC surface).  As a cell it:

- holds the root `Job`, the root `Namespace`, and the device capabilities it needs;
- uses the same `Job`/`File`/`Device` invocations as any other cell;
- has no private syscall, no shared kernel control block and no raw kernel callback;
- gates `POST /mcp` with the bearer token from `mcp.token` (AGENTS.md);
- has no audit in v0.1; the DS1386 watchdog is the recovery path.

A bug in the MCP cell is a kernel-level bug because it holds root.  The capability model
limits what a compromised *non-root* cell can do; it does not contain a compromised MCP.

---

## 9. Conformance and implementation slices

### 9.1 Layout and constants

The `native-abi` crate gains the cell-side types (it remains `no_std`, allocation-free
and `unsafe`-free):

- `CellStart`, `CellCap`, `CellFd`, `CellAux`, `CellEvent`, `CellManifest`;
- `CellFileStat`, `CellDirEntry`, `CellSockaddr`, `CellFileInfo`, `CellDeviceInfo`,
  `CellThreadInfo`;
- `CapHandle`, `Rights`, `ObjectType`, `Status`, `InvokeSelector`;
- the method blocks of §4.7 and the constants of §4.2/§4.3/§5.3.

The crate asserts `size_of`, `align_of` and `offset_of!` for every structure on the host
and on the n32 target.  A C oracle (`native-abi.h`) prints the same values; the outputs
are diffed.  The crate is the single layout source; the libc's `libc` `irix7` module and
the C header are generated from it or checked against it.

### 9.2 Unit and fuzz tests

| Area | Tests |
|---|---|
| Capability table | alloc/lookup/revoke, generation wrap, stale handle, rights attenuation, table full |
| Revocation | derivation tree, cross-cell delegation, waiter wakeup, `Device` child revocation |
| `CellStart` | every field valid, every checklist failure, random bytes never panic |
| Invocation block | size/version/alignment, overflow, reserved non-zero, trailing bytes preserved |
| Pointer validation | `MEM`/`DEV` class, VMAR ownership, direction, cross-VMAR, concurrent unmap |
| Namespace | `..` ceiling, absolute symlink, mount crossing, cross-namespace dir, `SYMLOOP_MAX` |
| fd table | dup shares offset, close refcount, `CLOEXEC`/`CLOFORK`, `F_DUPFD`, `EMFILE` |
| Futex | wait/wake, shared VMO key, timeout, signal interruption, lost wake |
| Status mapping | every `CELL_E_*` maps to a documented `errno`; no unmapped status |

Fuzz targets use arbitrary bytes for `CellStart`, `CellManifest`, invocation blocks and
path strings.  The invariant is: no panic, no out-of-bounds access, no hang; every
invalid input returns a status.  The fuzz corpus is checked in or generated
deterministically.

### 9.3 Cell loader

The M9 loader slice:

1. Parse ELF32 big-endian `ET_REL`, the section table and `.irix7.cell`.
2. Validate the manifest, the ABI version, the entry and the relocation set.
3. Allocate a `Job`, a cap table, a namespace and a contiguous image extent.
4. Copy sections, zero `.bss`, apply `R_MIPS_32/26/HI16/LO16/GPREL16`.
5. Map text RX, rodata R, data/bss RW, stack RW, the 4 KiB gap and the start page R.
6. Build `CellStart`, `CellCap[]`, `CellFd[]`, `argv`/`envp`/`auxv`, `libc_data`.
7. Write back the data cache and invalidate the instruction cache.
8. Enter `_start` and reap the exit status.

The loader is tested with synthetic images that cover a good image, a bad manifest, an
unsupported relocation, an out-of-range entry, overlapping sections, a missing `_start`
and a truncated section table.  No real native binary is committed until the SDK can
build one.

### 9.4 Trap smoke test

The asm gate test and the Rust open/read test of §5.7.  The negative tests are as
important as the positive ones: a bad gate, a forged handle, a stale generation, a bad
pointer, an oversized block and a missing right must fail cleanly.

### 9.5 Native libc v0

The first libc slice (M9):

- startup: `_start`, `CellStart` parsing, TCB, `errno`, the fd table, the standard fds;
- files: `open`, `openat`, `close`, `read`, `write`, `lseek`, `fstat`, `stat`, `mmap`,
  `munmap`, `mprotect`, `msync`;
- process: `posix_spawn`, `execve`, `waitpid`, `_exit`, `getpid`, `getppid`;
- time/random: `clock_gettime`, `nanosleep`, `getrandom`/`getentropy`;
- threads: `pthread_create`, `pthread_join`, `pthread_detach`, `pthread_key_*`,
  `pthread_mutex_*`, `pthread_cond_*` over `SYS_FUTEX_*` (M9.5 for the full set);
- errors: `__errno_location`, `strerror`, `strerror_r`;
- capability mapping: every call checks the handle and the rights; no ambient authority.

`fork`/`_Fork` return `ENOSYS` and are documented as deferred.

### 9.6 `std` PAL

The M9.5 slice is the target patch from the build spike (`std/build.rs`, `os::irix7`,
`MetadataExt`, `raw`, args, fs times, `__errno_location`, `getexecname`, `getentropy`,
`pthread_setname_np`) plus the native runtime:

- `_start`/`lang_start`, `catch_unwind`, `_Unwind_*` or the `unwinding` crate;
- pthreads over `Thread`, futex-based `std::sync`;
- `std::fs` over the `*at` family and `posix_getdents`/`readdir`;
- `std::net` over `Socket`, `poll`/`ppoll` readiness;
- `std::process::Command` over `posix_spawn` + `posix_spawn_file_actions_addchdir`;
- `std::env::current_exe` over `getexecname`;
- TLS over pthread keys (`has-thread-local: false`);
- the conformance suite on iris, not just a cross-build.

The kernel, ROM and modules must contain no `std` in their dependency graphs (D8, D17).

### 9.7 M9 and M9.5 mapping

| Slice | M9 | M9.5 | Post-M9.5 |
|---|---|---|---|
| SAS kernel-mode cells | yes | yes | mapped cells open |
| `CellStart` v0.1 | yes | yes | minor additions |
| Capability table, rights, derive/drop/revoke | yes | yes | |
| Gate and invocation encoding | yes | yes | |
| `Job`, `File`, `Namespace`, `Vmar`, `Vmo`, `Channel`, `Port` | yes | yes | |
| `Socket`, `Device`, `Mmio`, `Irq`, `DmaWindow` | basic | full | |
| System invocations (clock, random, sleep, futex) | clock/random/sleep | futex | |
| Threads | create/join/exit | full pthreads, cancel | |
| TLS pthread keys | yes | yes | ELF TLS |
| Signals | async at safe points; faults terminate | synchronous faults, `sigaltstack` | kernel-assisted delivery |
| `fork` | `E_NOTSUP` | `E_NOTSUP` | COW with mapped cells |
| `posix_spawn`/`exec` | basic | file actions/attrs | |
| `std` PAL | no | yes | |
| `poll`/readiness | basic | full | |
| Per-cell address space / user mode | no | no | yes |

---

## 10. Open questions

These are the decisions this draft does **not** freeze.  The recommended default is what
M9 should implement unless review says otherwise.  Do not treat any row as settled.

| # | Decision | Recommended default | Owner | Blocks |
|---:|---|---|---|---|
| 1 | Per-cell address space vs SAS | SAS kseg0 for M9; mapped kseg2/kuseg for untrusted code and `fork` | kernel | post-M9.5 |
| 2 | User mode vs kernel-mode cells | Kernel mode for M9; user mode only with a mapped address space | kernel | post-M9.5 |
| 3 | Is `fork` required in M9? | No; `posix_spawn`/`exec`; `fork` returns `ENOSYS` | kernel / ABI | M9 |
| 4 | Cell image format and relocation set | ELF32 BE `ET_REL` subset; `R_MIPS_32/26/HI16/LO16/GPREL16` | kernel / toolchain | M9 |
| 5 | `CellStart` final size and fields | 160 bytes; append-only; §3.1 | ABI | M9 |
| 6 | Handle packing | `(generation << 32) \| index`; null 0 | ABI | M9 |
| 7 | Rights bit set | 10 bits; reserve the rest; bit 31 kernel-only | ABI | M9 |
| 8 | Object type numbers | 0–13 as in §4.3; reserve the rest | ABI | M9 |
| 9 | Invocation selector | `(type << 16) \| method` | ABI | M9 |
| 10 | Native status vs `errno` | Separate `CELL_E_*`; libc maps to errno | ABI | M9 |
| 11 | Signal delivery | Deferred at safe points in M9; kernel-assisted later | kernel / ABI | M9.5 |
| 12 | Synchronous faults | Terminate in M9; kernel `ucontext_t` in M9.5 | kernel | M9.5 |
| 13 | TLS/TP | `$28`; pthread keys v0.1; ELF TLS later | ABI / loader | M9.5 |
| 14 | `dup`/`fork` offset sharing | Offset in the kernel `File` object; fd table in libc | ABI | M9 |
| 15 | Namespace default | Global namespace cap granted by `init`; private optional | kernel | M9 |
| 16 | `posix_spawn` file actions | Executed by the libc; kernel receives the fd plan | ABI / libc | M9 |
| 17 | Shared libc vs static | Static per cell in v0.1; shared post-v1 | ABI | post-v1 |
| 18 | `poll` readiness model | `Port`-based registration; M9.5 | kernel / libc | M9.5 |
| 19 | MCP audit | None in v0.1; future audit `Port` | infra | post-v1 |
| 20 | Capability table size | 256 default; 4096 cap | kernel | M9 |
| 21 | Auxv key set | §3.4 recommended set | ABI / loader | M9 |
| 22 | `getcwd` canonical path | Libc string; stale on rename; `PATH_OF` later | ABI / libc | M9.5 |
| 23 | `AT_HWCAP` bits | MIPS III/LL-SC/64-bit/BE; exact bits open | kernel / ABI | M9 |
| 24 | Quotas | §8.3 defaults; per-job policy | kernel | M9 |
| 25 | `abi_stable` for cell-to-cell | Not the kernel boundary; optional inside a cell protocol | ABI | post-v1 |
| 26 | Gate code `0x1F7` | Keep; confirm no IRIX conflict | kernel | M9 |
| 27 | `Thread`/`Namespace` as objects | Keep separate; revisit if the object model grows | ABI | M9 |
| 28 | `Device` as socket factory | Keep; a network cell proxy is an implementation detail | kernel | M9.5 |
| 29 | `fork` reparent policy | Reparent to `init`, POSIX-like | kernel | M9.5 |
| 30 | Core dumps | None in v0.1 | kernel | post-v1 |

---

## 11. Conflicts with existing specifications

These are places where this draft differs from, or sharpens, an existing document.  None
is a silent change; each should be resolved in a follow-up edit or ADR.

| # | Document | Existing statement | This draft | Recommended resolution |
|---:|---|---|---|---|
| 1 | [`architecture.md`](architecture.md) §5.3 | The call surface uses `abi_stable` (candidate) or `repr(C)` | The kernel boundary is `repr(C)`; `abi_stable` is not used for handles or invocations | Keep `abi_stable` for cell-to-cell Rust protocols only; amend §5.3 |
| 2 | [`architecture.md`](architecture.md) §5.4 | "File descriptor: a capability handle in the process's table" | The fd table is libc state; the kernel has no native fd table | Clarify that "the process's table" is the libc's; amend §5.4 |
| 3 | [`architecture.md`](architecture.md) §5.5 | "The libc is a cell; it can be updated independently" | The v0.1 libc is static per cell | State that a shared libc cell is post-v1 |
| 4 | [`architecture.md`](architecture.md) §5.4 and [`../plan/milestones.md`](../plan/milestones.md) M9 | `fork` clones the cell; M9 demonstrates `fork`/`exec` | General `fork` is not implementable in kseg0 SAS; M9 uses `posix_spawn`/`exec` | Amend M9's criterion; keep `fork` as the mapped-cell goal |
| 5 | [`architecture.md`](architecture.md) §4.2 / D17 | Drivers are "de-privileged cells, not ring-3 processes" | Cells run in kernel mode; de-privileged means authority-limited, not less CPU privilege | Define the term in §4.2; amend the table note |
| 6 | [`native-libc.md`](native-libc.md) §4.6 | "Native code does not execute the IRIX `syscall` instruction" | Native code executes `syscall 0x1F7`, a different code with a different convention | Reword to "does not use the IRIX syscall convention or numbers" |
| 7 | [`native-libc.md`](native-libc.md) §4.6 | "The libc is the only component that performs capability invocations" | Any cell may invoke; the libc is the POSIX path | Reword to "the libc is the only POSIX component" |
| 8 | [`native-libc.md`](native-libc.md) §4.5 / §9 | The `CellStart` layout is open | §3 fixes the v0.1 layout | Update the cross-reference; keep the libc out of the layout |
| 9 | [`native-libc.md`](native-libc.md) §5.1 / §4.7 | `fork` is in the v1 profile | `fork` returns `ENOSYS` until mapped cells | Mark `fork` deferred in the libc profile |
| 10 | [`elf.md`](elf.md) §2 / §7 | The loader rejects `ET_REL`; it is the IRIX compatibility contract | The native cell loader needs `ET_REL` | Keep `elf.md` compat-only; write a separate native cell parser |
| 11 | [`posix.md`](posix.md) §9.2 | A cell has a "user MMU context" | M9 cells are kernel-mode SAS | Amend to "kernel-mode SAS in M9; user MMU context when cells are mapped" |
| 12 | [`architecture.md`](architecture.md) §17.4 | Open: invocation encoding, `abi_stable` vs `repr(C)`, `fork`, guard pages, SAS risk, raw MMIO | This draft answers encoding/`abi_stable`/`fork`/guard pages; raw MMIO remains a driver policy | Record the answers; leave raw MMIO to the driver spec |
| 13 | [`architecture.md`](architecture.md) §5.2 | The object list omits `Thread` | `Thread` is a separate object | Add `Thread` to §5.2 or accept this draft as the extension |
| 14 | [`architecture.md`](architecture.md) §5.2 | `Namespace` is optional and the default process has a global namespace | Every cell has a `Namespace` handle; it may be empty | Clarify that the handle always exists; the authority may be empty |
| 15 | [`native-libc.md`](native-libc.md) §4.7 | The loader provides "one guard page" below the stack | kseg0 SAS has no guard page; the loader leaves a 4 KiB gap that is not a VMAR but cannot fault | Document that `pthread_attr_getguardsize` is advisory; stack-overflow detection is M9.5/mapped cells |
| 16 | [`native-libc.md`](native-libc.md) §4.5 | `_start` reads `argc`/`argv`/`envp`/`auxv` from the System V stack | `CellStart` carries them; the libc may build the stack shape for a C runtime | Make `CellStart` authoritative; the libc constructs the stack if needed |

The two most consequential conflicts are **#4 (`fork`)** and **#6 (the trap instruction)**.
The first changes the M9 demo; the second is a wording fix in the libc spec.

---

## 12. References

- [`architecture.md`](architecture.md) — D1–D32, §4 (architecture), §5 (native ABI),
  §6 (compatibility container), §17.4 (kernel/ABI open questions).
- [`native-libc.md`](native-libc.md) — the n32 C ABI, POSIX profile, `errno`, TLS,
  startup and capability enforcement above this contract.
- [`posix.md`](posix.md) — the POSIX.1-2024 source baseline and ABI obligations.
- [`elf.md`](elf.md) — the IRIX compatibility ELF loader (not the native loader).
- [`driver-interface.md`](driver-interface.md) — `ctl`/`data`/`status`, `posix_devctl`
  and the device-class payloads.
- [`modules.md`](modules.md) — the separate kernel-module service table.
- [`../plan/interfaces.md`](../plan/interfaces.md) — interface register and statuses.
- [`../plan/milestones.md`](../plan/milestones.md) — M9 and M9.5.
- [`../research/rust-std-irix7.md`](../research/rust-std-irix7.md) — the target matrix,
  §5.4 capability enforcement, §7.2–§7.5 the PAL and cell runtime, §9.3 the PR sequence.
- [`../research/rust-std-build-spike.md`](../research/rust-std-build-spike.md) — the
  original compile-only `std` rlib and patch set.
- [`../research/std-hello-bringup.md`](../research/std-hello-bringup.md) — the first
  running static `std` image and the bring-up libc/fd/syscall additions.
- [`../../crates/native-abi/README.md`](../../crates/native-abi/README.md) — the layout
  source and its provisional status.
- [`../../kernel/src/capability.rs`](../../kernel/src/capability.rs),
  [`../../kernel/src/traps.rs`](../../kernel/src/traps.rs),
  [`../../kernel/src/tlb.rs`](../../kernel/src/tlb.rs),
  [`../../kernel/src/scheduler.rs`](../../kernel/src/scheduler.rs) — the current kernel
  types this ABI extends.
- ADR-0011, *Native interface is a capability/object model*.
- ADR-0021, *Native Rust `std` is load-bearing; its libc baseline is POSIX.1-2024*.
- The Open Group, *POSIX.1-2024 / Base Specifications, Issue 8*.

---

## Appendix A: Kernel-side payload structures

These are kernel ABI structures, not the native libc's C layouts.  The libc converts
them to `struct stat`, `struct dirent`, `struct sockaddr` and so on.  They are
`repr(C)`, big-endian and 8-byte aligned unless noted.

### A.1 `CellFileStat` (112 bytes)

```c
struct CellFileStat {
    uint64_t dev;          /* 0x00 */
    uint64_t ino;          /* 0x08 */
    uint32_t mode;         /* 0x10 */
    uint32_t nlink;        /* 0x14 */
    uint32_t uid;          /* 0x18 */
    uint32_t gid;          /* 0x1C */
    uint64_t rdev;         /* 0x20 */
    uint64_t size;         /* 0x28 */
    uint32_t blksize;      /* 0x30 */
    uint32_t reserved0;    /* 0x34 */
    uint64_t blocks;       /* 0x38 */
    int64_t  atime_sec;    /* 0x40 */
    uint32_t atime_nsec;   /* 0x48 */
    uint32_t reserved1;    /* 0x4C */
    int64_t  mtime_sec;    /* 0x50 */
    uint32_t mtime_nsec;   /* 0x58 */
    uint32_t reserved2;    /* 0x5C */
    int64_t  ctime_sec;    /* 0x60 */
    uint32_t ctime_nsec;   /* 0x68 */
    uint32_t reserved3;    /* 0x6C */
};                          /* 0x70 = 112 */
```

The libc maps this to its 128-byte `struct stat`; the kernel does not depend on the
libc's layout.

### A.2 `CellDirEntry` (32 bytes + name)

```c
struct CellDirEntry {
    uint64_t ino;        /* 0x00 */
    uint64_t off;        /* 0x08: cookie for the next READDIR */
    uint32_t reclen;     /* 0x10: 32 + name_len, rounded to 8 */
    uint32_t type;       /* 0x14: CELL_DT_* */
    uint32_t name_len;   /* 0x18 */
    uint32_t reserved;   /* 0x1C */
    char     name[];     /* 0x20: not NUL-terminated */
};
```

`READDIR` fills a buffer with consecutive records; `reclen` is the stride.  The libc
converts to `struct dirent`/`struct posix_dent`, adding the NUL and applying `NAME_MAX`.
`CELL_DT_*` follows the native libc's `DT_*` values: 0 unknown, 1 fifo, 2 chr, 4 dir,
6 blk, 8 reg, 10 lnk, 12 sock, 14 whiteout.

### A.3 `CellSockaddr` (32 bytes, 4-aligned)

```c
struct CellSockaddr {
    uint16_t family;      /* 0x00: CELL_AF_* */
    uint16_t port;        /* 0x02: network byte order */
    uint8_t  addr[16];    /* 0x04: IPv6; IPv4 in addr[0..4] */
    uint32_t scope_id;    /* 0x14 */
    uint32_t flowinfo;    /* 0x18 */
    uint32_t reserved;    /* 0x1C */
};                          /* 0x20 = 32 */
```

The libc converts between this and `sockaddr_in`/`sockaddr_in6`.  `CELL_AF_UNIX` is 1,
`CELL_AF_INET` is 2 and `CELL_AF_INET6` is 10; other values follow the native libc.  The
kernel does not use the libc's `sockaddr` layouts.

### A.4 `CellManifest` (128 bytes)

```c
struct CellManifest {
    uint32_t magic;         /* 0x00: 0x4937434D "I7CM" */
    uint16_t abi_major;     /* 0x04 */
    uint16_t abi_minor;     /* 0x06 */
    uint32_t flags;         /* 0x08 */
    uint32_t entry;         /* 0x0C: offset from image_base, or 0 for _start */
    uint32_t stack_size;    /* 0x10 */
    uint32_t min_cap_slots; /* 0x14 */
    uint64_t build_id;      /* 0x18 */
    char     name[64];      /* 0x20: NUL-padded */
    uint8_t  reserved[32];  /* 0x60 */
};                            /* 0x80 = 128 */
```

The manifest is in the `.irix7.cell` section.  `build_id` is for diagnostics and updates,
not a signature; image integrity comes from the boot bundle's SHA-256 (D26) or a future
signed manifest.

### A.5 `CellDeviceInfo` (72 bytes)

```c
struct CellDeviceInfo {
    uint32_t device_class; /* 0x00: 1 net, 2 display, 3 audio, 4 block, 5 serial, 6 input, 7 rtc, 8 other */
    uint32_t vendor;       /* 0x04 */
    uint32_t device;       /* 0x08 */
    uint32_t mmio_count;   /* 0x0C */
    uint32_t irq_count;    /* 0x10 */
    uint32_t dma_max;      /* 0x14 */
    char     name[32];     /* 0x18 */
    uint8_t  reserved[16]; /* 0x38 */
};                            /* 0x48 = 72, 4-aligned */
```

### A.6 `CellThreadInfo` (64 bytes)

```c
struct CellThreadInfo {
    uint32_t tid;          /* 0x00 */
    uint32_t state;        /* 0x04: 1 running, 2 runnable, 3 blocked, 4 exited */
    uint32_t stack_base;   /* 0x08 */
    uint32_t stack_size;   /* 0x0C */
    uint32_t tls;          /* 0x10 */
    uint32_t sched_class;  /* 0x14: 1 fair, 2 EDF */
    uint64_t cpu_ns;       /* 0x18 */
    uint64_t deadline;     /* 0x20 */
    uint32_t name_len;     /* 0x28 */
    uint32_t reserved;     /* 0x2C */
    char     name[16];     /* 0x30 */
};                            /* 0x40 = 64 */
```

### A.7 `CellFileInfo` (32 bytes)

```c
struct CellFileInfo {
    uint32_t type;        /* 0x00: 1 reg, 2 dir, 3 symlink, 4 chr, 5 blk, 6 fifo, 7 sock */
    uint32_t flags;       /* 0x04: CELL_FILE_* */
    uint64_t size;        /* 0x08 */
    uint32_t blksize;     /* 0x10 */
    uint32_t reserved;    /* 0x14 */
    uint64_t reserved2;   /* 0x18 */
};                          /* 0x20 = 32 */
```

`CELL_FILE_TTY` (bit 0), `CELL_FILE_SEEKABLE` (bit 1), `CELL_FILE_READABLE` (bit 2),
`CELL_FILE_WRITABLE` (bit 3).

### A.8 `CellStatVfs` (optional)

Deferred; the libc can synthesise `statvfs` from `CellFileInfo` and the namespace.  A
future `File::STATVFS` returns a documented structure.

---

## Appendix B: Worked example — `open` and `read`

A native cell with `CELL_SELF_HANDLE` in slot 1 and `CELL_NAMESPACE_HANDLE` in slot 2:

1. The libc builds `CellNamespaceOpen { dir = 0, path = ptr("etc/motd"), path_len = 8,
   flags = O_RDONLY, mode = 0 }` in its own memory.
2. It issues `syscall 0x1F7` with `$v0 = CELL_INVOKE(12, 1)`, `$a0 =
   CELL_NAMESPACE_HANDLE`, `$a1 = block`, `$a2 = 24`, `$a3 = 1`.
3. The kernel resolves the namespace, checks `INVOKE` on it, resolves the path inside
   the root, checks the resulting `File`'s rights, and returns `$v0 = CELL_OK`, `$v1 =
   file_handle`.
4. The libc allocates the lowest free fd and stores `FdEntry { handle = file_handle,
   refcount = 1, flags = 0 }`.
5. `read(fd, buf, 128)` builds `CellFileIo { buf, len = 128, offset = 0, flags = 0 }` and
   issues `CELL_INVOKE(6, 1)` with `$a0 = file_handle`.  The kernel checks `READ`,
   validates the buffer, reads from the `File`'s current offset (0), advances it, and
   returns the byte count in `$v1`.
6. `close(fd)` decrements the reference; if it reaches zero, the libc issues
   `Job::CAP_DROP(file_handle)`.  The `File` object is destroyed when its last
   capability drops.

If the namespace root lacks `READ`, step 3 returns `CELL_E_ACCESS`; the libc maps it to
`EACCES`.  If the path escapes the namespace, step 3 returns `CELL_E_ACCESS`; a symlink
loop returns `CELL_E_LOOP`.  If the buffer is outside the cell's VMAR, step 5 returns
`CELL_E_FAULT`; the libc maps it to `EFAULT` and treats it as a caller bug.

## Appendix C: Worked example — `posix_spawn`

1. The libc starts from the parent's fd table and applies the file actions: `addopen`
   opens `/etc/motd` read-only, `adddup2` maps it to fd 3, `addchdir` resolves `/tmp` to
   a `File`.
2. It builds `CellFd[] = { {0, 0, stdin_handle}, {1, 0, stdout_handle},
   {2, 0, stderr_handle}, {3, 0, motd_handle} }` in its own memory.
3. It builds `CellJobSpawn { image = image_handle, namespace_handle = 0, cwd = tmp_handle,
   argv, argc, envp, envc, fds, fd_count = 4, libc_data, libc_data_size, flags = 0,
   deadline = 0 }`.
4. The kernel checks `MANAGE` on the parent `Job` and `EXECUTE` on the image, creates the
   child `Job`, loads the image, rewrites the child's `CellFd[]` with the child's
   handles, installs slots 1/2 and the fd caps, and starts `_start`.
5. The parent's `$v1` is the child `Job` handle; `pid_out` is the child pid.  The libc
   records them and returns the pid from `posix_spawn`.
6. The child's libc parses its `CellStart`, builds its fd table from `CellFd[]`, and
   calls `main`.  The open file descriptions (and offsets) are shared with the parent,
   as POSIX requires.

If the image lacks `EXECUTE`, step 4 returns `CELL_E_ACCESS` before creating the child.
If a `CellFd` handle lacks `TRANSFER`, step 4 returns `CELL_E_PERM` and no child is
created.  If the child's table is full, step 4 returns `CELL_E_NOMEM`.
