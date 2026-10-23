// SPDX-License-Identifier: GPL-3.0-only
//! On-disk constants from the ELF gABI and the MIPS psABI.
//!
//! Only the values needed to recognise and load an IRIX o32 or n32 image are
//! listed.  A program header whose `p_type` is not named here is preserved as
//! [`crate::SegmentType::Other`] rather than being discarded.

/// The ELF magic number.
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// Number of bytes in the ELF identification field.
pub const EI_NIDENT: usize = 16;

/// `EI_CLASS` value for a 32-bit ELF object.
pub const ELFCLASS32: u8 = 1;

/// `EI_DATA` value for a big-endian object.
pub const ELFDATA2MSB: u8 = 2;

/// `EI_VERSION` and `e_version` value for the current format.
pub const EV_CURRENT: u32 = 1;

/// `e_machine` value for the MIPS architecture.
pub const EM_MIPS: u16 = 8;

/// `e_type` value for no file type.
pub const ET_NONE: u16 = 0;

/// `e_type` value for a relocatable object.
pub const ET_REL: u16 = 1;

/// `e_type` value for an executable.
pub const ET_EXEC: u16 = 2;

/// `e_type` value for a position-independent object or shared library.
pub const ET_DYN: u16 = 3;

/// `e_type` value for a core file.
pub const ET_CORE: u16 = 4;

/// A program header that is unused.
pub const PT_NULL: u32 = 0;

/// A loadable segment.
pub const PT_LOAD: u32 = 1;

/// The dynamic linking information.
pub const PT_DYNAMIC: u32 = 2;

/// The program interpreter path.
pub const PT_INTERP: u32 = 3;

/// A note segment.
pub const PT_NOTE: u32 = 4;

/// A reserved segment type.
pub const PT_SHLIB: u32 = 5;

/// The program header table itself.
pub const PT_PHDR: u32 = 6;

/// Thread-local storage.
pub const PT_TLS: u32 = 7;

/// MIPS register information.
pub const PT_MIPS_REGINFO: u32 = 0x7000_0000;

/// MIPS ABI options.
pub const PT_MIPS_OPTIONS: u32 = 0x7000_0001;

/// MIPS ABI flags.
pub const PT_MIPS_ABIFLAGS: u32 = 0x7000_0002;

/// Segment is executable.
pub const PF_X: u32 = 1;

/// Segment is writable.
pub const PF_W: u32 = 2;

/// Segment is readable.
pub const PF_R: u32 = 4;

/// The MIPS file uses the n32 ABI.
pub const EF_MIPS_ABI2: u32 = 0x0000_0020;

/// Mask for the ABI field in `e_flags`.
pub const EF_MIPS_ABI: u32 = 0x0000_f000;

/// The MIPS file uses the o32 ABI.
pub const EF_MIPS_ABI_O32: u32 = 0x0000_1000;

/// The MIPS file uses the o64 ABI.
pub const EF_MIPS_ABI_O64: u32 = 0x0000_2000;

/// The MIPS file uses the EABI32 ABI.
pub const EF_MIPS_ABI_EABI32: u32 = 0x0000_3000;

/// The MIPS file uses the EABI64 ABI.
pub const EF_MIPS_ABI_EABI64: u32 = 0x0000_4000;

/// Size of an ELF32 file header.
pub const EHDR_SIZE: usize = 52;

/// Size of the fields this crate consumes from an ELF32 program header.
pub const PHDR_SIZE: usize = 32;

/// Size of the fields this crate consumes from an ELF32 section header.
pub const SHDR_SIZE: usize = 40;

/// Largest program-header entry size accepted.
///
/// The ELF32 entry is 32 bytes.  A larger entry is tolerated only up to this
/// bound so that a corrupt `e_phentsize` cannot turn a small image into a
/// multi-gigabyte table by multiplication.
pub const MAX_PHDR_ENTRY_SIZE: u16 = 1024;

/// Largest section-header entry size accepted.
pub const MAX_SHDR_ENTRY_SIZE: u16 = 1024;
