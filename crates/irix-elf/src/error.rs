// SPDX-License-Identifier: GPL-3.0-only
//! Errors returned by the IRIX ELF parser.

use core::fmt;

/// An IRIX ELF parsing or validation error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is shorter than an ELF32 file header.
    Truncated,
    /// The `\x7fELF` magic is missing.
    BadMagic,
    /// `EI_CLASS` is not `ELFCLASS32`.
    NotElf32,
    /// `EI_DATA` is not `ELFDATA2MSB`.
    NotBigEndian,
    /// `EI_VERSION` is not [`EV_CURRENT`](crate::constants::EV_CURRENT).
    BadIdentVersion,
    /// `e_version` is not [`EV_CURRENT`](crate::constants::EV_CURRENT).
    BadHeaderVersion,
    /// `e_ehsize` is smaller than an ELF32 file header.
    InvalidHeaderSize(u16),
    /// `e_type` is not `ET_EXEC` or `ET_DYN`.
    UnsupportedElfType(u16),
    /// `e_machine` is not `EM_MIPS`.
    UnsupportedMachine(u16),
    /// `e_phentsize` is zero, too small or unreasonably large.
    InvalidProgramHeaderSize(u16),
    /// The program-header table lies partly or wholly outside the image.
    ProgramHeaderTableOutOfBounds,
    /// A program header's file range lies outside the image.
    SegmentOutOfBounds(usize),
    /// A program header's file or memory range overflows 32 bits.
    SegmentOverflow(usize),
    /// A program header has `p_filesz > p_memsz`.
    FileSizeExceedsMemorySize(usize),
    /// A program header's `p_align` is neither zero nor a power of two.
    InvalidAlignment(usize),
    /// A `PT_LOAD` segment's `p_vaddr` and `p_offset` are not congruent
    /// modulo `p_align`.
    MisalignedSegment(usize),
    /// A `PT_INTERP` segment has an empty path.
    EmptyInterp(usize),
    /// A `PT_INTERP` segment has no NUL terminator.
    UnterminatedInterp(usize),
    /// The image contains more than one `PT_INTERP` segment.
    DuplicateInterp,
    /// A load plan was requested for an image with no `PT_LOAD` segments.
    NoLoadableSegments,
    /// The section-header table is inconsistent or uses extended numbering.
    InvalidSectionTable,
    /// The section-header table lies partly or wholly outside the image.
    SectionHeaderTableOutOfBounds,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("image is shorter than the ELF32 file header"),
            Self::BadMagic => f.write_str("not an ELF image"),
            Self::NotElf32 => f.write_str("not an ELF32 image"),
            Self::NotBigEndian => f.write_str("ELF image is not big-endian"),
            Self::BadIdentVersion => f.write_str("unsupported ELF identification version"),
            Self::BadHeaderVersion => f.write_str("unsupported ELF header version"),
            Self::InvalidHeaderSize(size) => write!(f, "invalid ELF header size {size}"),
            Self::UnsupportedElfType(kind) => write!(f, "unsupported ELF type {kind}"),
            Self::UnsupportedMachine(machine) => {
                write!(f, "unsupported ELF machine {machine}, expected EM_MIPS (8)")
            }
            Self::InvalidProgramHeaderSize(size) => {
                write!(f, "invalid program-header entry size {size}")
            }
            Self::ProgramHeaderTableOutOfBounds => {
                f.write_str("program-header table is outside the image")
            }
            Self::SegmentOutOfBounds(index) => {
                write!(
                    f,
                    "program header {index} has a file range outside the image"
                )
            }
            Self::SegmentOverflow(index) => {
                write!(f, "program header {index} has an overflowing range")
            }
            Self::FileSizeExceedsMemorySize(index) => {
                write!(
                    f,
                    "program header {index} has p_filesz greater than p_memsz"
                )
            }
            Self::InvalidAlignment(index) => {
                write!(
                    f,
                    "program header {index} has a p_align that is not zero or a power of two"
                )
            }
            Self::MisalignedSegment(index) => {
                write!(
                    f,
                    "PT_LOAD segment {index} has p_vaddr and p_offset that are not congruent modulo p_align"
                )
            }
            Self::EmptyInterp(index) => {
                write!(f, "PT_INTERP segment {index} has an empty path")
            }
            Self::UnterminatedInterp(index) => {
                write!(f, "PT_INTERP segment {index} is not NUL-terminated")
            }
            Self::DuplicateInterp => f.write_str("image has more than one PT_INTERP segment"),
            Self::NoLoadableSegments => f.write_str("image has no PT_LOAD segments"),
            Self::InvalidSectionTable => {
                f.write_str("section-header table is invalid or uses extended numbering")
            }
            Self::SectionHeaderTableOutOfBounds => {
                f.write_str("section-header table is outside the image")
            }
        }
    }
}

impl core::error::Error for Error {}

/// A result from the IRIX ELF parser.
pub type Result<T> = core::result::Result<T, Error>;
