// SPDX-License-Identifier: GPL-3.0-only
//! Program headers and segment permissions.

use core::fmt;
use core::iter::FusedIterator;
use core::ops::Range;

use crate::constants::{
    PF_R, PF_W, PF_X, PHDR_SIZE, PT_DYNAMIC, PT_INTERP, PT_LOAD, PT_MIPS_ABIFLAGS, PT_MIPS_OPTIONS,
    PT_MIPS_REGINFO, PT_NOTE, PT_NULL, PT_PHDR, PT_SHLIB, PT_TLS,
};
use crate::read::u32_at;

/// The `p_type` of a program header.
///
/// Types this crate does not name are preserved exactly as
/// [`SegmentType::Other`].  A loader can therefore ignore an unknown segment
/// without losing the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentType {
    /// An unused program header.
    Null,
    /// A loadable segment.
    Load,
    /// Dynamic linking information.
    Dynamic,
    /// The program interpreter path.
    Interp,
    /// A note segment.
    Note,
    /// A reserved segment type.
    Shlib,
    /// The program header table.
    Phdr,
    /// Thread-local storage.
    Tls,
    /// MIPS register information.
    MipsReginfo,
    /// MIPS ABI options.
    MipsOptions,
    /// MIPS ABI flags.
    MipsAbiflags,
    /// A type this crate does not name.
    Other(u32),
}

impl SegmentType {
    /// Decode a `p_type` field.
    pub const fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::Null,
            1 => Self::Load,
            2 => Self::Dynamic,
            3 => Self::Interp,
            4 => Self::Note,
            5 => Self::Shlib,
            6 => Self::Phdr,
            7 => Self::Tls,
            PT_MIPS_REGINFO => Self::MipsReginfo,
            PT_MIPS_OPTIONS => Self::MipsOptions,
            PT_MIPS_ABIFLAGS => Self::MipsAbiflags,
            other => Self::Other(other),
        }
    }

    /// The raw `p_type` value.
    pub const fn raw(self) -> u32 {
        match self {
            Self::Null => PT_NULL,
            Self::Load => PT_LOAD,
            Self::Dynamic => PT_DYNAMIC,
            Self::Interp => PT_INTERP,
            Self::Note => PT_NOTE,
            Self::Shlib => PT_SHLIB,
            Self::Phdr => PT_PHDR,
            Self::Tls => PT_TLS,
            Self::MipsReginfo => PT_MIPS_REGINFO,
            Self::MipsOptions => PT_MIPS_OPTIONS,
            Self::MipsAbiflags => PT_MIPS_ABIFLAGS,
            Self::Other(value) => value,
        }
    }
}

/// The permissions encoded by `p_flags`.
///
/// Unknown flag bits are ignored here but remain available through
/// [`ProgramHeader::flags`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    /// The segment is readable (`PF_R`).
    pub read: bool,
    /// The segment is writable (`PF_W`).
    pub write: bool,
    /// The segment is executable (`PF_X`).
    pub execute: bool,
}

impl Permissions {
    /// Decode the permission bits of a `p_flags` value.
    pub const fn from_flags(flags: u32) -> Self {
        Self {
            read: flags & PF_R != 0,
            write: flags & PF_W != 0,
            execute: flags & PF_X != 0,
        }
    }

    /// Encode the permissions as `p_flags` bits.
    pub const fn bits(self) -> u32 {
        let mut bits = 0;
        if self.read {
            bits |= PF_R;
        }
        if self.write {
            bits |= PF_W;
        }
        if self.execute {
            bits |= PF_X;
        }
        bits
    }

    /// Whether no permission is set.
    pub const fn is_empty(self) -> bool {
        !self.read && !self.write && !self.execute
    }
}

/// A validated program header.
///
/// The `file` slice is bounded by the image supplied to
/// [`Elf::parse`](crate::Elf::parse); its length is always `p_filesz`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProgramHeader<'a> {
    p_type: u32,
    p_offset: u32,
    p_vaddr: u32,
    p_paddr: u32,
    p_filesz: u32,
    p_memsz: u32,
    p_flags: u32,
    p_align: u32,
    file: &'a [u8],
}

impl fmt::Debug for ProgramHeader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgramHeader")
            .field("p_type", &self.p_type)
            .field("p_offset", &self.p_offset)
            .field("p_vaddr", &self.p_vaddr)
            .field("p_paddr", &self.p_paddr)
            .field("p_filesz", &self.p_filesz)
            .field("p_memsz", &self.p_memsz)
            .field("p_flags", &self.p_flags)
            .field("p_align", &self.p_align)
            .field("file_len", &self.file.len())
            .finish()
    }
}

impl<'a> ProgramHeader<'a> {
    pub(crate) fn from_raw(raw: RawProgramHeader, file: &'a [u8]) -> Self {
        Self {
            p_type: raw.p_type,
            p_offset: raw.p_offset,
            p_vaddr: raw.p_vaddr,
            p_paddr: raw.p_paddr,
            p_filesz: raw.p_filesz,
            p_memsz: raw.p_memsz,
            p_flags: raw.p_flags,
            p_align: raw.p_align,
            file,
        }
    }

    pub(crate) fn parse(image: &'a [u8], offset: usize) -> Option<Self> {
        let raw = RawProgramHeader::read(image, offset)?;
        let start = usize::try_from(raw.p_offset).ok()?;
        let size = usize::try_from(raw.p_filesz).ok()?;
        let end = start.checked_add(size)?;
        let file = image.get(start..end)?;
        Some(Self::from_raw(raw, file))
    }

    /// The decoded `p_type`.
    pub const fn kind(&self) -> SegmentType {
        SegmentType::from_raw(self.p_type)
    }

    /// The raw `p_type`.
    pub const fn raw_type(&self) -> u32 {
        self.p_type
    }

    /// The file offset of the segment.
    pub const fn file_offset(&self) -> u32 {
        self.p_offset
    }

    /// The virtual address of the segment.
    pub const fn virtual_address(&self) -> u32 {
        self.p_vaddr
    }

    /// The physical address of the segment.
    ///
    /// IRIX images normally set this equal to `p_vaddr`.
    pub const fn physical_address(&self) -> u32 {
        self.p_paddr
    }

    /// The number of bytes present in the file.
    pub const fn file_size(&self) -> u32 {
        self.p_filesz
    }

    /// The number of bytes the segment occupies in memory.
    pub const fn memory_size(&self) -> u32 {
        self.p_memsz
    }

    /// The raw `p_flags`.
    pub const fn flags(&self) -> u32 {
        self.p_flags
    }

    /// The decoded permissions.
    pub const fn permissions(&self) -> Permissions {
        Permissions::from_flags(self.p_flags)
    }

    /// The raw `p_align`.
    pub const fn alignment(&self) -> u32 {
        self.p_align
    }

    /// The segment bytes in the image.
    ///
    /// The length is exactly [`file_size`](Self::file_size).
    pub const fn file(&self) -> &'a [u8] {
        self.file
    }

    /// The segment's byte range in the image.
    pub fn file_range(&self) -> Range<usize> {
        let start = self.p_offset as usize;
        start..start + self.file.len()
    }

    /// The first address after the segment's memory image.
    ///
    /// The sum was checked against 32-bit overflow during parsing.
    pub const fn memory_end(&self) -> u32 {
        self.p_vaddr + self.p_memsz
    }
}

/// A cursor over the validated program-header table.
///
/// The iterator re-reads headers from the image; it does not allocate.  Every
/// header was validated by [`Elf::parse`](crate::Elf::parse), so iteration
/// cannot produce an out-of-bounds file slice.
#[derive(Clone, Debug)]
pub struct ProgramHeaders<'a> {
    image: &'a [u8],
    offset: usize,
    entry_size: usize,
    remaining: usize,
}

impl<'a> ProgramHeaders<'a> {
    pub(crate) fn new(image: &'a [u8], offset: usize, entry_size: usize, count: usize) -> Self {
        Self {
            image,
            offset,
            entry_size,
            remaining: count,
        }
    }
}

impl<'a> Iterator for ProgramHeaders<'a> {
    type Item = ProgramHeader<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let Some(header) = ProgramHeader::parse(self.image, self.offset) else {
            // The table was validated at parse time, so this is unreachable
            // for an `Elf`.  End the iteration rather than panic if the
            // invariant is ever broken.
            self.remaining = 0;
            return None;
        };
        self.offset += self.entry_size;
        self.remaining -= 1;
        Some(header)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ProgramHeaders<'_> {
    fn len(&self) -> usize {
        self.remaining
    }
}

impl FusedIterator for ProgramHeaders<'_> {}

/// The fields read from one ELF32 program header.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RawProgramHeader {
    pub p_type: u32,
    pub p_offset: u32,
    pub p_vaddr: u32,
    pub p_paddr: u32,
    pub p_filesz: u32,
    pub p_memsz: u32,
    pub p_flags: u32,
    pub p_align: u32,
}

impl RawProgramHeader {
    pub(crate) fn read(image: &[u8], offset: usize) -> Option<Self> {
        let end = offset.checked_add(PHDR_SIZE)?;
        let raw = image.get(offset..end)?;
        Some(Self {
            p_type: u32_at(raw, 0)?,
            p_offset: u32_at(raw, 4)?,
            p_vaddr: u32_at(raw, 8)?,
            p_paddr: u32_at(raw, 12)?,
            p_filesz: u32_at(raw, 16)?,
            p_memsz: u32_at(raw, 20)?,
            p_flags: u32_at(raw, 24)?,
            p_align: u32_at(raw, 28)?,
        })
    }
}
