# irix-elf

A `no_std` parser and load-plan view for IRIX o32 and n32 ELF images.

The crate is the first dependency of the M5 legacy container.  It reads a
32-bit big-endian MIPS ELF image, validates every field and range it consumes,
and exposes the program headers and `PT_LOAD` segments that a kernel loader
needs.  It does not map memory, allocate frames, resolve symbols, parse
relocations or follow the program interpreter.

It has no allocation, no external dependencies and no unsafe code.  An `Elf`
borrows the caller's image, so parsing costs no copies.

```rust
use irix_elf::Elf;

let image = read_the_file()?;
let elf = Elf::parse(&image)?;
let plan = elf.load_plan()?;

println!("entry {:#010x}, ABI {:?}", plan.entry(), elf.abi());
if let Some(path) = plan.interp() {
    println!("interpreter {}", core::str::from_utf8(path)?);
}

for segment in plan.segments() {
    // Copy segment.file_data() to segment.load_address(), then zero
    // segment.zero_fill() bytes after it.  Page rounding, address
    // translation and MMU permissions are the caller's job.
    let _ = (
        segment.load_address(),
        segment.memory_size(),
        segment.permissions(),
    );
}
```

## API

| Item | Purpose |
|---|---|
| `Elf::parse(&[u8])` | Validate the header, the program-header table and every segment |
| `Elf::program_headers()` | Iterator over all program headers, including unknown types |
| `Elf::program_header(index)` | One header by table index |
| `Elf::entry()`, `Elf::abi()`, `Elf::flags()` | Header values and the MIPS ABI derived from `e_flags` |
| `Elf::interp()` | `PT_INTERP` path bytes, without the NUL terminator |
| `Elf::dynamic()` | The `PT_DYNAMIC` header, if present |
| `Elf::load_plan()` | Validated `PT_LOAD` view, or `NoLoadableSegments` |
| `LoadPlan::segments()` | Iterator over `Segment` views in program-header order |
| `Segment::load_address()`, `memory_size()`, `zero_fill()` | Mapping arithmetic |
| `Segment::file_data()`, `file_range()`, `permissions()` | Bytes and access bits |
| `LoadPlan::extent()` | Lowest load address and highest memory end |

Unknown program-header types are preserved as `SegmentType::Other(raw)`; the
loader can ignore them without losing the header.  `PT_INTERP` is returned as
bytes because IRIX paths are not required to be UTF-8.

## Validation

Parsing rejects, with a typed error:

- a short image, bad magic, ELF64 class or little-endian data;
- an unsupported identification or header version;
- a non-MIPS machine, or an `e_type` other than `ET_EXEC`/`ET_DYN`;
- an `e_ehsize` smaller than 52 bytes;
- a zero, smaller-than-32 or larger-than-1024 `e_phentsize` when there are
  program headers;
- a program-header table that overruns the image;
- a program header whose file range overflows 32 bits or lies outside the
  image;
- `p_filesz > p_memsz`, or a memory range that overflows 32 bits;
- a `p_align` that is neither zero nor a power of two, and a `PT_LOAD` whose
  `p_vaddr` and `p_offset` are not congruent modulo `p_align`;
- an empty `PT_INTERP`, one without a NUL terminator, or more than one
  `PT_INTERP`;
- a section-header table that is inconsistent, uses extended numbering or
  overruns the image.

Section contents are not parsed.  The section-header table is validated only
because the header fields name it.

## Deliberate limitations

- No dynamic linking, relocation or symbol resolution; `PT_DYNAMIC` is exposed
  as bytes and left for a later layer.
- No ECOFF; the PROM bootstrap uses ECOFF and is not an M5 ELF concern.
- No section-based loading, overlap detection or page-size rounding.  A caller
  that maps overlapping `PT_LOAD` segments must decide which one wins.
- Only `ET_EXEC` and `ET_DYN` images are accepted; `ET_REL` and `ET_CORE` are
  rejected rather than misinterpreted.
- Little-endian and ELF64 images are rejected: every IRIX o32 and n32 binary is
  ELF32 big-endian.

The full contract is in [`docs/spec/elf.md`](../../docs/spec/elf.md), and the
container context is in [`docs/spec/architecture.md`](../../docs/spec/architecture.md)
§6.

## Tests

The fifty-six host tests build synthetic images in memory; no real IRIX
binary is committed.  They cover the o32 and n32-like headers, multiple
`PT_LOAD` segments, BSS zero fill, `PT_INTERP`, `PT_DYNAMIC`, unknown types,
truncated headers, bad class, data and machine, overflowing and out-of-bounds
ranges, `p_filesz > p_memsz`, unaligned `PT_LOAD` segments, overlapping load
segments, malformed section tables, and the public load-plan view.

Run them with:

```sh
cargo test -p irix-elf
```
