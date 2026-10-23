# IRIX ELF loader contract

**Status:** draft
**Version:** 0.1
**Owner:** compatibility
**Implementation:** [`crates/irix-elf`](../../crates/irix-elf)
**Related:** [`architecture.md`](architecture.md) §6, [`../plan/milestones.md`](../plan/milestones.md) M5.

## 1. Scope

This document fixes the ELF parsing and load-plan contract used by the M5
legacy container.  The crate is a parser, not a process loader: it validates
an image and exposes the segments to map.  It does not allocate frames, build
page tables, resolve symbols, relocate, or start a process.  Syscall decoding
and the o32 runtime are separate work.

IRIX 6.5 applications are ELF.  Both o32 and n32 use **ELF32** (32-bit
pointers); they differ in register width, argument conventions and `e_flags`.
The container accepts an unchanged executable image and leaves the ABI
personality to the process layer.

## 2. Accepted image profile

An image is accepted only when all of the following hold:

| Property | Required value |
|---|---|
| Magic | `0x7f 'E' 'L' 'F'` |
| `EI_CLASS` | `ELFCLASS32` (1) |
| `EI_DATA` | `ELFDATA2MSB` (2) |
| `EI_VERSION` | `EV_CURRENT` (1) |
| `e_type` | `ET_EXEC` (2) or `ET_DYN` (3) |
| `e_machine` | `EM_MIPS` (8) |
| `e_version` | `EV_CURRENT` (1) |
| `e_ehsize` | at least 52 |
| `e_phentsize` | 32..=1024 when `e_phnum` is non-zero |
| `e_shentsize` | 40..=1024 when a section-header table is present |

Little-endian, ELF64, `ET_REL`, `ET_CORE`, ECOFF and non-MIPS images are
rejected rather than reinterpreted.  `EI_OSABI` and `EI_ABIVERSION` are
exposed but not constrained: IRIX binaries usually declare `ELFOSABI_SYSV`
(0), and `ELFOSABI_IRIX` is value 8.

A larger-than-32-byte program-header entry is tolerated up to the 1024-byte
bound; only the first 32 bytes are consumed and the entry stride is honoured.
A zero `e_phentsize` with `e_phnum == 0` is harmless because no header is read.

## 3. File header

The parser reads the 52-byte ELF32 file header at offset 0.  Every offset is
read through a bounds-checked big-endian accessor.

| Field | Offset | Contract |
|---|---:|---|
| `e_ident` | 0 | Magic, class, data and identification version checked; `EI_OSABI` and `EI_ABIVERSION` exposed |
| `e_type` | 16 | `ET_EXEC` or `ET_DYN` |
| `e_machine` | 18 | `EM_MIPS` |
| `e_version` | 20 | `EV_CURRENT` |
| `e_entry` | 24 | Exposed as the entry virtual address |
| `e_phoff` | 28 | Table start; checked with `e_phnum` and `e_phentsize` |
| `e_shoff` | 32 | Section table start, or zero |
| `e_flags` | 36 | Exposed; the MIPS ABI is decoded from it |
| `e_ehsize` | 40 | At least 52 |
| `e_phentsize` | 42 | Program-header entry size |
| `e_phnum` | 44 | Program-header count |
| `e_shentsize` | 46 | Section-header entry size, when present |
| `e_shnum` | 48 | Section-header count, when present |
| `e_shstrndx` | 50 | Ignored; section names are not read |

The program-header table range is `e_phoff + e_phnum * e_phentsize`, computed
with checked arithmetic and required to lie entirely inside the image.

### 3.1 Section headers

Sections are not needed to load an image.  The table is validated only because
the file header names it:

- `e_shoff == 0` requires `e_shnum == 0`;
- `e_shoff != 0` requires `e_shnum != 0`, a valid entry size and a table that
  fits in the image;
- ELF extended numbering (`e_shnum == 0` with a non-zero `e_shoff`) is
  rejected because the real count lives in section header zero;
- section contents, names and `e_shstrndx` are not parsed.

### 3.2 MIPS ABI classification

`e_flags` is decoded, not rejected:

| Condition | ABI |
|---|---|
| `(e_flags & EF_MIPS_ABI) == EF_MIPS_ABI_O32` | o32 |
| `(e_flags & EF_MIPS_ABI) == EF_MIPS_ABI_O64` | o64 |
| `(e_flags & EF_MIPS_ABI) == EF_MIPS_ABI_EABI32` | EABI32 |
| `(e_flags & EF_MIPS_ABI) == EF_MIPS_ABI_EABI64` | EABI64 |
| ABI field zero and `EF_MIPS_ABI2` set | n32 |
| Anything else | unknown |

The M5 demo is o32.  n32 is represented and tested because the same parser
serves both; the o32/n32 personality is chosen later from this value.

## 4. Program headers

Each 32-byte ELF32 program header is decoded:

| Field | Offset |
|---|---:|
| `p_type` | 0 |
| `p_offset` | 4 |
| `p_vaddr` | 8 |
| `p_paddr` | 12 |
| `p_filesz` | 16 |
| `p_memsz` | 20 |
| `p_flags` | 24 |
| `p_align` | 28 |

The loader consumes `PT_LOAD` (1), `PT_INTERP` (3) and `PT_DYNAMIC` (2).
Other types are preserved as a raw value; a loader may ignore them.

Validation applies to every header, not only `PT_LOAD`:

1. `p_offset + p_filesz` must not overflow 32 bits.
2. The file range must lie inside the image, so `p_offset + p_filesz` is at
   most the image length.  This includes zero-length ranges: `p_offset` itself
   must be at most the image length.
3. `p_filesz` must not exceed `p_memsz`.
4. `p_vaddr + p_memsz` must not overflow 32 bits.
5. `p_align` must be zero, one, or a power of two.
6. A `PT_LOAD` with `p_align > 1` must satisfy `p_vaddr % p_align ==
   p_offset % p_align`.

`p_offset`, `p_vaddr`, `p_paddr`, `p_filesz`, `p_memsz`, `p_flags` and
`p_align` are all exposed.  `PF_R` (4), `PF_W` (2) and `PF_X` (1) are decoded
into a `Permissions` view; unknown flag bits are preserved in the raw value.

### 4.1 `PT_INTERP`

The segment must contain a non-empty path terminated by a NUL byte within
`p_filesz`.  The byte after the first NUL is ignored.  The path is returned
without its terminator as bytes, because IRIX paths are not required to be
UTF-8.  An image with more than one `PT_INTERP` is rejected.

Following the interpreter is a later layer.  This contract exposes the path.

### 4.2 `PT_DYNAMIC`

The segment's file and memory ranges are validated like any other header, and
the header is exposed.  Dynamic entries are **not** decoded, and no relocation
or symbol table is read.  The M5 demo may run a statically linked binary; the
dynamic path is deliberately left unimplemented.

## 5. Load plan

`Elf::load_plan()` requires at least one `PT_LOAD` segment and returns a view
over the loadable segments in **program-header order**.  Segments are not
sorted by address, merged, or rounded to a page size.

A `Segment` reports:

| Query | Meaning |
|---|---|
| `load_address()` | `p_vaddr` |
| `physical_address()` | `p_paddr`, normally equal to `p_vaddr` |
| `file_offset()` | `p_offset` |
| `file_size()` | `p_filesz` |
| `memory_size()` | `p_memsz` |
| `zero_fill()` | `p_memsz - p_filesz` |
| `file_data()` | The `p_filesz` bytes in the image |
| `file_range()` | The same bytes as a range in the image |
| `permissions()` | `PF_R`, `PF_W`, `PF_X` |
| `alignment()` | `p_align` |
| `memory_range()` | `load_address .. load_address + memory_size` |
| `memory_end()` | The first address after the memory image |

The mapping contract is: copy `file_data()` at `load_address`, then zero
`zero_fill()` bytes immediately after it.  The order in `LoadPlan::segments()`
is the order to use when segments share a page or overlap.

`LoadPlan::extent()` returns the lowest `load_address` and the highest
`memory_end`.  It is useful for sizing an address space, but it is not a
mapping instruction: a caller still maps each segment separately.

The plan is independent of the MMU.  kseg0 translation, page rounding, cache
maintenance and the choice of ASID belong to the kernel loader.  `e_entry` is
exposed as-is and is not required to fall inside an executable segment: an
`ET_DYN` shared object can have entry zero while still having loadable
segments.

## 6. Trust model

An image is untrusted input.  The parser:

- uses checked arithmetic for every offset, size and range;
- bounds every slice against the supplied image;
- never allocates, never copies the image and never uses `unsafe`;
- returns a typed `Error` rather than panicking on malformed input;
- reads only the fields listed above; no field is trusted because another
  field claims it is consistent.

The image remains owned by the caller.  An `Elf`, its iterators and its
`LoadPlan` borrow it and cannot outlive it.

## 7. Deliberate limitations

- No relocation, symbol resolution, dynamic linker or shared-library loading.
- No `PT_NOTE`, `PT_MIPS_REGINFO`, `PT_MIPS_OPTIONS` or `PT_TLS` decoding;
  these are preserved as headers and ignored.
- No section-based loading and no section contents.
- No overlap rejection.  Two `PT_LOAD` segments may describe overlapping
  memory; the kernel mapper decides the policy.
- No page-size rounding or kseg translation.
- No ECOFF, ELF64 or little-endian images.
- No verification that `e_entry` points into an executable segment.

These boundaries keep the crate a pure parser and keep MMU policy in the
loader that will consume it.

## 8. Testing and corpus

The crate has fifty-six host tests.  Every image is built in the test binary:
a valid o32 image (interpreter, two loadable segments, dynamic information and
an unnamed type), an n32-like header, BSS-only, overlapping and out-of-order
segments, and a series of single-field corruptions covering each rejection
above.

No real IRIX binary is committed.  The compatibility corpus is private; this
contract and the synthetic images are the public oracle.

Run the tests with:

```sh
cargo test -p irix-elf
```
